//! Every model the documents show loads with the real catalog, and those that
//! are whole models run (roadmap M4, M5): the README, the introduction and the
//! concept pages are read from the documents themselves, so a change there is
//! tested.

mod support;

use std::path::{Path, PathBuf};

use support::try_load;

fn repo(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        |p, q| p.join(q),
    )
}

/// Each YAML block of a Markdown file, with the label of the panel it stands in.
fn yaml_blocks(markdown: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    let mut label = String::new();
    for line in markdown.lines() {
        match (&mut current, line.trim_start()) {
            (None, "```yaml") => current = Some(String::new()),
            (Some(block), "```") => {
                blocks.push((label.clone(), std::mem::take(block)));
                current = None;
            }
            (Some(block), _) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, text) => {
                if text.contains("ts-label") {
                    text.clone_into(&mut label);
                }
            }
        }
    }
    blocks
}

/// What a page's fragment needs around it to be a whole model: a Source before
/// the components it shows and a Sink after them. `None` for what is no list of
/// components, such as a page's `flow` or `experiment` alone.
fn whole(block: &str) -> Option<String> {
    if block.starts_with("format:") {
        return Some(block.to_owned());
    }
    let list: Vec<&str> = block
        .lines()
        .skip_while(|line| *line == "components:")
        .take_while(|line| line.starts_with(' '))
        .collect();
    let ids: Vec<&str> = list
        .iter()
        .filter_map(|line| line.trim_start().strip_prefix("- id:"))
        .map(str::trim)
        .collect();
    if ids.is_empty() {
        return None;
    }
    Some(format!(
        "format: \"1.0\"\nmodel: {{name: fragment}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n  - {{id: source, type: Source, interarrival: 1min}}\n{}\n  - {{id: sink, type: Sink}}\n\
         flow:\n  - [source, {}, sink]\n",
        list.join("\n"),
        ids.join(", ")
    ))
}

#[test]
fn the_readme_and_the_introduction_load_and_run() {
    for file in [repo(&["README.md"]), repo(&["docs", "introduction.md"])] {
        let text = std::fs::read_to_string(&file).unwrap();
        let (_, block) = &yaml_blocks(&text)[0];
        let model = support::load(block);
        let r = support::run_model(&model, 42);
        assert!(r.report.items.delivered.get() > 100, "{}", file.display());
    }
}

#[test]
fn every_v0_1_model_on_a_concept_page_loads() {
    let dir = repo(&["docs", "concepts"]);
    let mut pages: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    pages.sort();
    let (mut loaded, mut waiting) = (0, 0);
    for page in &pages {
        let text = std::fs::read_to_string(page).unwrap();
        for (label, block) in yaml_blocks(&text) {
            let Some(model) = whole(&block) else {
                // `flow:` alone, or a section of a later tier.
                waiting += 1;
                continue;
            };
            if label.contains("v0.2") {
                waiting += 1;
                continue;
            }
            if let Err(errors) = try_load(&model) {
                panic!("{}:\n{block}\n{errors}", page.display());
            }
            loaded += 1;
        }
    }
    // Pages 1, 2, 4, 6, 7, 8 and 11, and 12's loop; pages 9, 10, 12 and 13 show something
    // else. What a note on a page quotes is tested below.
    assert_eq!((loaded, waiting), (8, 4));
}

#[test]
fn page_12_runs_its_rework_loop() {
    let text = std::fs::read_to_string(repo(&["docs", "concepts", "12-scrap-rework.md"])).unwrap();
    let block = yaml_blocks(&text)
        .into_iter()
        .map(|(_, block)| block)
        .find(|block| block.starts_with("format:"))
        .unwrap();
    let r = support::run_model(&support::load(&block), 42);
    assert!(r.report.items.delivered.get() > 500);
    assert_ne!(
        r.times("weld", |m| matches!(
            m,
            taktsim_components::FlowMessage::Return(_)
        ))
        .len(),
        0
    );
    support::check_records(&r);
}

#[test]
fn the_routing_examples_on_the_pages_load() {
    // Page 9's flow, on components of its own.
    let text = std::fs::read_to_string(repo(&["docs", "concepts", "09-longer-lines.md"])).unwrap();
    let start = text.find("> flow:\n").unwrap();
    let end = text[start..].find("> ```").unwrap();
    let flow: String = text[start..start + end]
        .lines()
        .map(|line| line.strip_prefix("> ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let machine = |id: &str| format!("  - {{id: {id}, type: Machine, cycle_time: 10s}}\n");
    let components: String = ["cut", "weld-1", "weld-2", "paint", "pack-1", "pack-2"]
        .map(machine)
        .concat();
    let model = format!(
        "format: \"1.0\"\nmodel: {{name: page, seed: 1}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n  - {{id: source, type: Source, variants: {{basic: 80%, premium: 20%}}}}\n{components}  \
         - {{id: sink, type: Sink}}\n{flow}\n"
    );
    let r = support::run_model(&support::load(&model), 1);
    assert!(r.arrival_variants("paint").iter().all(|v| v == "premium"));
    assert!(r.report.items.delivered.get() > 100);
    // Page 11's `control`, on one machine.
    let text =
        std::fs::read_to_string(repo(&["docs", "concepts", "11-batching-setups.md"])).unwrap();
    assert!(text.contains("dispatch: { weld: minimum_setup }"));
    let model = "format: \"1.0\"\nmodel: {name: page}\nexperiment: {type: terminating, horizon: 1h}\n\
         components:\n  - {id: source, type: Source, variants: {basic: 80%, premium: 20%}}\n  \
         - {id: weld, type: Machine, cycle_time: 45s, setup: {per_variant: 4min}}\n  - {id: sink, type: Sink}\n\
         flow:\n  - [source, weld, sink]\ncontrol:\n  dispatch: { weld: minimum_setup }\n";
    support::load(model);
}

#[test]
fn page_1_gives_its_table() {
    let text =
        std::fs::read_to_string(repo(&["docs", "concepts", "01-production-line.md"])).unwrap();
    let (_, block) = &yaml_blocks(&text)[0];
    let r = support::run(block, 42);
    assert_eq!(r.report.items.delivered.get(), 639);
    assert_eq!(r.delivered()[0].0, support::t("87s"));
}
