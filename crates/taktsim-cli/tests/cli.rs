//! Runs the real `taktsim` binary.

use std::process::Command;

fn taktsim(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .args(args)
        .output()
}

#[test]
fn version_prints_package_version() {
    for flag in ["--version", "-V"] {
        let out = taktsim(&[flag]).unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            format!("taktsim {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn help_prints_usage() {
    for flag in ["--help", "-h"] {
        let out = taktsim(&[flag]).unwrap();
        assert!(out.status.success());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .starts_with("usage: taktsim")
        );
    }
}

#[test]
fn unknown_arguments_fail_with_usage_on_stderr() {
    for args in [&[][..], &["run"][..], &["--version", "extra"][..]] {
        let out = taktsim(args).unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "");
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .starts_with("usage: taktsim")
        );
    }
}
