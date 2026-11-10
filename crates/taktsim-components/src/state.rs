//! The states of a machine and a Source, and which changes are allowed
//! (ADR-0022 §5, §7).

/// What a machine is doing. Computed from what it holds, never set by hand,
/// so it cannot disagree with it; the states partition time, so shares sum to
/// one (ADR-0022 §5).
///
/// ```
/// use taktsim_components::MachineState;
///
/// assert_eq!(MachineState::Down.code(), 4);
/// assert_eq!(MachineState::from_code(4), Some(MachineState::Down));
/// assert_eq!(MachineState::from_code(6), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MachineState {
    /// Waits for a part, and a Source reaches it through non-stations only: no
    /// demand.
    Idle,
    /// At least one unit is in its cycle.
    Busy,
    /// No unit in a cycle, at least one finished unit waiting: no room
    /// downstream.
    Blocked,
    /// Waits for a part, and every path from a Source passes a station: the
    /// line let it down.
    Starved,
    /// Failed, repair running. Nothing moves in or out.
    Down,
    /// A changeover is running.
    Setup,
}

impl MachineState {
    /// The state's code in the trace (ADR-0022 §9).
    ///
    /// ```
    /// use taktsim_components::MachineState;
    ///
    /// let codes = [MachineState::Idle, MachineState::Busy, MachineState::Blocked,
    ///     MachineState::Starved, MachineState::Down, MachineState::Setup].map(MachineState::code);
    /// assert_eq!(codes, [0, 1, 2, 3, 4, 5]);
    /// ```
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Busy => 1,
            Self::Blocked => 2,
            Self::Starved => 3,
            Self::Down => 4,
            Self::Setup => 5,
        }
    }

    /// The state with `code`, if there is one.
    ///
    /// ```
    /// use taktsim_components::MachineState;
    ///
    /// assert_eq!(MachineState::from_code(0), Some(MachineState::Idle));
    /// assert_eq!(MachineState::from_code(255), None);
    /// ```
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Idle,
            1 => Self::Busy,
            2 => Self::Blocked,
            3 => Self::Starved,
            4 => Self::Down,
            5 => Self::Setup,
            _ => return None,
        })
    }

    /// The states this one may change to (ADR-0022 §5). `busy` never goes
    /// straight to `idle` or `starved`: a finished part always passes through
    /// `blocked`, for zero time if there is room. `idle` and `starved` never
    /// turn into each other.
    ///
    /// ```
    /// use taktsim_components::MachineState::{Blocked, Busy, Down, Idle, Setup};
    ///
    /// assert_eq!(Busy.successors(), [Blocked, Setup, Down]);
    /// assert!(!Busy.successors().contains(&Idle));
    /// ```
    #[must_use]
    pub const fn successors(self) -> &'static [Self] {
        match self {
            Self::Idle | Self::Starved => &[Self::Busy, Self::Setup, Self::Down],
            Self::Setup => &[Self::Busy, Self::Down],
            Self::Busy => &[Self::Blocked, Self::Setup, Self::Down],
            Self::Blocked => &[
                Self::Busy,
                Self::Setup,
                Self::Idle,
                Self::Starved,
                Self::Down,
            ],
            Self::Down => &[
                Self::Idle,
                Self::Starved,
                Self::Busy,
                Self::Blocked,
                Self::Setup,
            ],
        }
    }

    /// The state's name, as in the model's documents.
    ///
    /// ```
    /// assert_eq!(taktsim_components::MachineState::Starved.as_str(), "starved");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Blocked => "blocked",
            Self::Starved => "starved",
            Self::Down => "down",
            Self::Setup => "setup",
        }
    }
}

/// What a Source is doing. A Buffer has no state; its fill says everything.
///
/// ```
/// use taktsim_components::SourceState;
///
/// assert_eq!(SourceState::from_code(1), Some(SourceState::Blocked));
/// assert_eq!(SourceState::Blocked.code(), 1);
/// assert_eq!(SourceState::from_code(2), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceState {
    /// Its interval runs, or it makes parts on demand. It holds nothing.
    Waiting,
    /// It holds parts that have not left.
    Blocked,
}

impl SourceState {
    /// The state's code in the trace (ADR-0022 §9).
    ///
    /// ```
    /// assert_eq!(taktsim_components::SourceState::Waiting.code(), 0);
    /// ```
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Blocked => 1,
        }
    }

    /// The state with `code`, if there is one.
    ///
    /// ```
    /// use taktsim_components::SourceState;
    ///
    /// assert_eq!(SourceState::from_code(0), Some(SourceState::Waiting));
    /// ```
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Waiting),
            1 => Some(Self::Blocked),
            _ => None,
        }
    }

    /// The state's name.
    ///
    /// ```
    /// assert_eq!(taktsim_components::SourceState::Blocked.as_str(), "blocked");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
        }
    }
}
