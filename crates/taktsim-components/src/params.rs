//! Value types of the factory vocabulary shared by several component types,
//! with the rules each adds to its base type (ADR-0022, "The vocabulary").

use taktsim_model::expr::ComponentId;
use taktsim_model::{
    ComponentRef, Decode, Decoder, Distribution, Encode, Json, Location, Node, Schemas,
};
use taktsim_units::{Count, SimDuration};

/// The most parts of a batch, the highest capacity of a machine, and the most
/// places an on-demand Source reaches through buffers alone: a batch is created
/// in one instant and an on-demand Source fills those buffers at time zero, so
/// more could exhaust memory on one PC and not on another, and a model that
/// loads must run the same everywhere.
pub(crate) const MAX_UNITS: u64 = 65_535;

/// The longest variant name in bytes, so its note always fits a record.
pub(crate) const MAX_NAME: usize = 255;

/// `batch` and a machine's `capacity`: above zero, at most 65,535 parts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Units(pub(crate) Count);

impl Decode for Units {
    const EXPECTED: &'static str = "a count from 1 to 65535 parts, such as `2 parts`";
    const EXAMPLE: &'static str = "2 parts";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let count = Count::decode(node, cx)?;
        if !(1..=MAX_UNITS).contains(&count.get()) {
            cx.not_allowed(
                node,
                count.to_string(),
                "from 1 to 65535 parts: all of them are created or hold a place in one instant",
                Self::EXAMPLE,
            );
            return None;
        }
        Some(Self(count))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        schemas
            .of::<Count>()
            .with("description", &"From 1 to 65535 parts.")
    }
}

/// A Buffer's `capacity`: above zero. A buffer of zero places is a direct edge.
/// With where it is written, for the check of what an on-demand Source fills
/// (ADR-0022, "The vocabulary").
#[derive(Debug, Clone, Copy)]
pub(crate) struct Places(pub(crate) Count, pub(crate) Location);

impl Decode for Places {
    const EXPECTED: &'static str = "a count of at least 1 parts, such as `20 parts`";
    const EXAMPLE: &'static str = "20 parts";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let count = Count::decode(node, cx)?;
        if count.get() == 0 {
            cx.not_allowed(
                node,
                count.to_string(),
                "at least 1 parts; a buffer of no places is a direct edge",
                Self::EXAMPLE,
            );
            return None;
        }
        Some(Self(count, node.start()))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        schemas.of::<Count>().with(
            "description",
            &"The places of the buffer, at least 1. A buffer of no places is a direct edge. The \
              buffers an on-demand Source reaches through buffers alone hold at most 65535 \
              places together.",
        )
    }
}

/// A distribution that is not a constant `0s`: a repeating zero-length timer
/// never lets time advance, and a constant zero cycle is not a machine.
fn not_zero<T: Decode>(node: &Node, cx: &mut Decoder) -> Option<Distribution> {
    let distribution = Distribution::decode(node, cx)?;
    if distribution == Distribution::Constant(SimDuration::ZERO) {
        cx.not_allowed(
            node,
            distribution.to_string(),
            "a duration longer than 0s, or a distribution",
            T::EXAMPLE,
        );
        return None;
    }
    Some(distribution)
}

/// A distribution with a description in the schema; `$not_zero` also refuses a
/// constant `0s`.
macro_rules! duration_parameter {
    ($(#[$doc:meta])* $name:ident, $example:literal, $description:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy)]
        pub(crate) struct $name(pub(crate) Distribution);

        impl Decode for $name {
            const EXPECTED: &'static str = Distribution::EXPECTED;
            const EXAMPLE: &'static str = $example;

            fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
                not_zero::<Self>(node, cx).map(Self)
            }

            fn schema(schemas: &mut Schemas) -> Json {
                schemas.of::<Distribution>().with("description", &$description)
            }
        }
    };
}

duration_parameter!(
    /// A Source's `interarrival`.
    Interarrival,
    "{dist: exponential, mean: 40s}",
    "The time between batches, counted from the last part of the batch leaving. Absent: the \
     Source makes a part whenever the next component takes one. A bare duration is a constant."
);
duration_parameter!(
    /// A Machine's `cycle_time`.
    CycleTime,
    "{dist: lognormal, mean: 42s, sigma: 0.08}",
    "The time one unit takes, drawn when its cycle starts. A bare duration is a constant."
);
duration_parameter!(
    /// A Machine's `mtbf`.
    Mtbf,
    "{dist: exponential, mean: 6h}",
    "The up time from the end of a repair (or time zero) to the next failure, in calendar time. \
     A bare duration is a constant: the machine fails on a fixed schedule, so name a \
     distribution unless regular failures are meant. Comes with `mttr`."
);
duration_parameter!(
    /// A Machine's `mttr`.
    Mttr,
    "{dist: exponential, mean: 12min}",
    "The time a repair takes, drawn at each failure. A bare duration is a constant. Comes with \
     `mtbf`."
);
duration_parameter!(
    /// A setup's `per_variant`.
    PerVariant,
    "4min",
    "The changeover time of any change of variant the `matrix` does not list."
);

/// A variant name in a value position: of the component-id form (ADR-0015), at
/// most 255 bytes, with its place for the check across the document.
#[derive(Debug, Clone)]
pub(crate) struct VariantRef(pub(crate) ComponentRef);

impl Decode for VariantRef {
    const EXPECTED: &'static str = "a variant name such as `premium`";
    const EXAMPLE: &'static str = "premium";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let name = String::decode(node, cx)?;
        if !is_variant_name(&name) {
            cx.not_allowed(node, name, VARIANT_NAME_RULE, Self::EXAMPLE);
            return None;
        }
        ComponentRef::decode(node, cx).map(Self)
    }

    fn schema(schemas: &mut Schemas) -> Json {
        schemas
            .of::<ComponentRef>()
            .with("description", &"A variant name, declared by some Source.")
    }
}

impl Encode for VariantRef {
    fn encode(&self) -> Json {
        self.0.encode()
    }
}

/// What a variant name may be.
pub(crate) const VARIANT_NAME_RULE: &str =
    "a name like a component id, such as `premium`, of at most 255 bytes";

/// Whether `name` may name a variant.
pub(crate) fn is_variant_name(name: &str) -> bool {
    name.len() <= MAX_NAME && ComponentId::new(name).is_some()
}
