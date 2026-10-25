//! The observation table: columnar, with a sorted dictionary of names.

use std::collections::{BTreeMap, BTreeSet};
use std::iter::FusedIterator;

use super::{Measurement, Name, SourceOffset, Timestamp, Unit};

/// One row of an [`ObservationTable`]: what a source reported about one
/// signal of one entity at one instant.
///
/// ```
/// use taktsim_adapters::observation::{Measurement, Name, Observation, SourceOffset, Timestamp, Unit};
///
/// let entity = Name::new("M1")?;
/// let signal = Name::new("state")?;
/// let observation = Observation {
///     timestamp: Timestamp::UNIX_EPOCH,
///     source_offset: SourceOffset::Unknown,
///     entity: &entity,
///     signal: &signal,
///     measurement: Measurement::new(2.0, Unit::One)?,
/// };
/// assert_eq!(observation.entity.as_str(), "M1");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation<'a> {
    /// When, as an exact UTC instant (column `timestamp`).
    pub timestamp: Timestamp,
    /// The UTC offset the source wrote the timestamp in (metadata).
    pub source_offset: SourceOffset,
    /// What was observed, e.g. a machine (column `entity`).
    pub entity: &'a Name,
    /// Which of its signals, e.g. a counter (column `signal`).
    pub signal: &'a Name,
    /// The value and its unit (columns `value` and `unit`).
    pub measurement: Measurement,
}

/// The internal observation format (spec §8): rows of `timestamp`,
/// `entity`, `signal`, `value` and `unit`, with the source's UTC offset per
/// row as metadata.
///
/// Rows are always sorted by timestamp, then entity, then signal; rows equal
/// in all three keep the order they were added in. So the table does not
/// depend on the order a source happened to write rows in, except for such
/// duplicates.
///
/// Storage is columnar, one vector per column, and names are
/// dictionary-encoded: each distinct entity or signal name is stored once.
/// This matches the Arrow and Parquet layout the table gets in v0.2
/// (ADR-0017).
///
/// ```
/// use taktsim_adapters::observation::{Measurement, Name, Observation, ObservationTable, Timestamp, Unit};
///
/// let m1 = Name::new("M1")?;
/// let count = Name::new("good_count")?;
/// let at = |text: &str| Timestamp::parse_rfc3339(text);
/// let row = |(timestamp, source_offset), value| -> Result<_, Box<dyn std::error::Error>> {
///     Ok(Observation {
///         timestamp,
///         source_offset,
///         entity: &m1,
///         signal: &count,
///         measurement: Measurement::new(value, Unit::Parts)?,
///     })
/// };
/// let table: ObservationTable = [
///     row(at("2026-03-02T07:00:00+01:00")?, 60.0)?,
///     row(at("2026-03-02T06:00:00+01:00")?, 0.0)?,
/// ]
/// .into_iter()
/// .collect();
/// let values: Vec<f64> = table.iter().map(|o| o.measurement.value()).collect();
/// assert_eq!(values, [0.0, 60.0]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObservationTable {
    timestamps: Vec<Timestamp>,
    source_offsets: Vec<SourceOffset>,
    /// Indexes into `names`.
    entities: Vec<usize>,
    /// Indexes into `names`.
    signals: Vec<usize>,
    values: Vec<f64>,
    units: Vec<Unit>,
    /// Every name used, sorted and distinct, so comparing indexes compares
    /// names.
    names: Vec<Name>,
}

impl ObservationTable {
    /// The empty table.
    ///
    /// ```
    /// use taktsim_adapters::observation::ObservationTable;
    ///
    /// assert!(ObservationTable::new().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of rows.
    ///
    /// ```
    /// use taktsim_adapters::observation::ObservationTable;
    ///
    /// assert_eq!(ObservationTable::new().len(), 0);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    /// Whether the table has no rows.
    ///
    /// ```
    /// use taktsim_adapters::observation::ObservationTable;
    ///
    /// assert!(ObservationTable::default().is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Row `index`, or `None` past the end.
    ///
    /// ```
    /// use taktsim_adapters::observation::ObservationTable;
    ///
    /// assert!(ObservationTable::new().get(0).is_none());
    /// ```
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Observation<'_>> {
        Some(Observation {
            timestamp: *self.timestamps.get(index)?,
            source_offset: *self.source_offsets.get(index)?,
            entity: self.names.get(*self.entities.get(index)?)?,
            signal: self.names.get(*self.signals.get(index)?)?,
            measurement: Measurement::from_finite(
                *self.values.get(index)?,
                *self.units.get(index)?,
            ),
        })
    }

    /// The rows in order.
    ///
    /// ```
    /// use taktsim_adapters::observation::ObservationTable;
    ///
    /// assert_eq!(ObservationTable::new().iter().count(), 0);
    /// ```
    #[must_use]
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            table: self,
            front: 0,
            back: self.len(),
        }
    }

    /// The distinct UTC offsets the sources used, in ascending order: the
    /// table's time-zone metadata. More than one appears, e.g., when data
    /// spans a daylight-saving change.
    ///
    /// ```
    /// use taktsim_adapters::csv::{ReadOptions, read_observations};
    ///
    /// let csv = "timestamp,entity,signal,value,unit\n\
    ///            2026-10-25T02:30:00+02:00,M1,state,1,1\n\
    ///            2026-10-25T02:30:00+01:00,M1,state,2,1\n";
    /// let table = read_observations(csv.as_bytes(), &ReadOptions::default())?.table;
    /// let offsets: Vec<String> = table.source_offsets().iter().map(ToString::to_string).collect();
    /// assert_eq!(offsets, ["+01:00", "+02:00"]);
    /// # Ok::<(), taktsim_adapters::csv::ReadError>(())
    /// ```
    #[must_use]
    pub fn source_offsets(&self) -> BTreeSet<SourceOffset> {
        self.source_offsets.iter().copied().collect()
    }
}

impl<'a> FromIterator<Observation<'a>> for ObservationTable {
    /// Collects rows into a table, sorted as described on
    /// [`ObservationTable`].
    fn from_iter<I: IntoIterator<Item = Observation<'a>>>(rows: I) -> Self {
        let mut builder = TableBuilder::default();
        for row in rows {
            builder.push(
                row.timestamp,
                row.source_offset,
                row.entity.as_str(),
                row.signal.as_str(),
                row.measurement,
            );
        }
        builder.finish()
    }
}

impl<'a> IntoIterator for &'a ObservationTable {
    type Item = Observation<'a>;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// The rows of an [`ObservationTable`], in order; made by
/// [`ObservationTable::iter`].
///
/// ```
/// use taktsim_adapters::observation::ObservationTable;
///
/// let table = ObservationTable::new();
/// let mut rows = table.iter();
/// assert_eq!(rows.len(), 0);
/// assert!(rows.next().is_none());
/// ```
#[derive(Debug, Clone)]
pub struct Iter<'a> {
    table: &'a ObservationTable,
    front: usize,
    back: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Observation<'a>;

    fn next(&mut self) -> Option<Observation<'a>> {
        if self.front == self.back {
            return None;
        }
        let row = self.table.get(self.front);
        self.front += 1;
        row
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        self.table.get(self.back)
    }
}

impl ExactSizeIterator for Iter<'_> {}

impl FusedIterator for Iter<'_> {}

/// Collects rows with names checked by the caller, then sorts them.
#[derive(Debug, Default)]
pub(crate) struct TableBuilder {
    /// Each name with the index it was first seen at.
    ids: BTreeMap<Name, usize>,
    timestamps: Vec<Timestamp>,
    source_offsets: Vec<SourceOffset>,
    entities: Vec<usize>,
    signals: Vec<usize>,
    values: Vec<f64>,
    units: Vec<Unit>,
}

impl TableBuilder {
    /// `entity` and `signal` must be valid names.
    pub(crate) fn push(
        &mut self,
        timestamp: Timestamp,
        source_offset: SourceOffset,
        entity: &str,
        signal: &str,
        measurement: Measurement,
    ) {
        let entity = self.intern(entity);
        let signal = self.intern(signal);
        self.timestamps.push(timestamp);
        self.source_offsets.push(source_offset);
        self.entities.push(entity);
        self.signals.push(signal);
        self.values.push(measurement.value());
        self.units.push(measurement.unit());
    }

    fn intern(&mut self, name: &str) -> usize {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.ids.len();
        self.ids.insert(Name::new_unchecked(name), id);
        id
    }

    pub(crate) fn finish(self) -> ObservationTable {
        // Renumber names in sorted order, so indexes compare like names.
        let mut rank = vec![0; self.ids.len()];
        let mut names = Vec::with_capacity(self.ids.len());
        for (sorted, (name, first_seen)) in self.ids.into_iter().enumerate() {
            // Ids are 0..len, each once, so every slot is filled.
            if let Some(slot) = rank.get_mut(first_seen) {
                *slot = sorted;
            }
            names.push(name);
        }
        let entities: Vec<usize> = self.entities.iter().map(|&id| rank[id]).collect();
        let signals: Vec<usize> = self.signals.iter().map(|&id| rank[id]).collect();
        // A stable sort keeps the insertion order of rows with equal keys.
        let mut order: Vec<usize> = (0..self.timestamps.len()).collect();
        order.sort_by_key(|&i| (self.timestamps[i], entities[i], signals[i]));
        ObservationTable {
            timestamps: order.iter().map(|&i| self.timestamps[i]).collect(),
            source_offsets: order.iter().map(|&i| self.source_offsets[i]).collect(),
            entities: order.iter().map(|&i| entities[i]).collect(),
            signals: order.iter().map(|&i| signals[i]).collect(),
            values: order.iter().map(|&i| self.values[i]).collect(),
            units: order.iter().map(|&i| self.units[i]).collect(),
            names,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::UtcOffset;

    fn name(text: &str) -> Name {
        Name::new(text).unwrap()
    }

    #[test]
    fn rows_are_sorted_by_time_entity_signal_then_insertion() {
        let (m1, m2, a, b) = (name("M1"), name("M2"), name("a"), name("b"));
        let row = |t, entity, signal, value| Observation {
            timestamp: Timestamp::from_unix_nanos(t),
            source_offset: SourceOffset::Known(UtcOffset::UTC),
            entity,
            signal,
            measurement: Measurement::new(value, Unit::One).unwrap(),
        };
        let table: ObservationTable = [
            row(2, &m2, &a, 1.0),
            row(1, &m2, &b, 2.0),
            row(1, &m2, &a, 3.0),
            row(1, &m1, &b, 4.0),
            row(2, &m2, &a, 5.0),
            row(0, &m2, &b, 6.0),
        ]
        .into_iter()
        .collect();
        let values: Vec<f64> = table.iter().map(|o| o.measurement.value()).collect();
        assert_eq!(values, [6.0, 4.0, 3.0, 2.0, 1.0, 5.0]);
        let back: Vec<f64> = table.iter().rev().map(|o| o.measurement.value()).collect();
        assert_eq!(back, [5.0, 1.0, 2.0, 3.0, 4.0, 6.0]);
        // Byte order: upper case before lower case.
        assert_eq!(table.names, [m1.clone(), m2.clone(), a.clone(), b.clone()]);
        let first = table.get(0).unwrap();
        assert_eq!((first.entity, first.signal), (&m2, &b));
        assert!(table.get(6).is_none());
        assert_eq!((&table).into_iter().len(), 6);
    }

    #[test]
    fn iterators_meet_in_the_middle() {
        assert!(ObservationTable::new().is_empty());
        let m1 = name("M1");
        let row = |t| Observation {
            timestamp: Timestamp::from_unix_nanos(t),
            source_offset: SourceOffset::Unknown,
            entity: &m1,
            signal: &m1,
            measurement: Measurement::new(0.0, Unit::Parts).unwrap(),
        };
        let table: ObservationTable = (0..3).map(row).collect();
        let mut it = table.iter();
        assert_eq!(it.next().map(|o| o.timestamp.as_unix_nanos()), Some(0));
        assert_eq!(it.next_back().map(|o| o.timestamp.as_unix_nanos()), Some(2));
        assert_eq!(it.size_hint(), (1, Some(1)));
        assert_eq!(it.next().map(|o| o.timestamp.as_unix_nanos()), Some(1));
        assert!(it.next().is_none());
        assert!(it.next_back().is_none());
        assert_eq!(
            table.source_offsets().into_iter().collect::<Vec<_>>(),
            [SourceOffset::Unknown]
        );
        assert_eq!(ObservationTable::new(), ObservationTable::default());
    }
}
