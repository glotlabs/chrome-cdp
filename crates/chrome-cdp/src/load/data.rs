//! A journey's data (plan 23.3): a CSV file (its header names the
//! columns) or a JSON array of objects, one row per iteration, as
//! `${row.<column>}`. Rows are taken in order across all users, or at
//! random; after the last, the rows start again (`wrap`) or the run ends
//! (`stop`, for accounts that can be used once).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::profile::{DataExhausted, DataOrder, DataSpec};
use super::user::Random;

/// A row: its value by column.
pub(crate) type Row = BTreeMap<String, String>;

/// A journey's rows, and where the users are in them.
#[derive(Debug)]
pub(crate) struct Data {
    /// The file, as the profile names it.
    pub(crate) file: String,
    /// The columns, in the file's order.
    pub(crate) columns: Vec<String>,
    rows: Vec<Arc<Row>>,
    order: DataOrder,
    exhausted: DataExhausted,
    /// The order rows are taken in under `random` and `stop`: each once.
    shuffled: Vec<usize>,
    /// Rows taken so far.
    taken: AtomicU64,
}

impl Data {
    /// Reads `spec`'s file; `seed` shuffles a `random` and `stop` order.
    pub(crate) fn load(spec: &DataSpec, seed: u64) -> Result<Self, String> {
        let text = std::fs::read_to_string(&spec.path)
            .map_err(|e| format!("cannot read data {}: {e}", spec.written))?;
        let json = spec
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        let (columns, rows) = if json {
            parse_json(&text)
        } else {
            parse_csv(&text)
        }
        .map_err(|e| format!("data {}: {e}", spec.written))?;
        if rows.is_empty() {
            return Err(format!("data {} has no rows", spec.written));
        }
        let mut shuffled: Vec<usize> = (0..rows.len()).collect();
        let mut random = Random::new(seed);
        for i in (1..shuffled.len()).rev() {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let j = ((random.unit() * (i + 1) as f64) as usize).min(i);
            shuffled.swap(i, j);
        }
        Ok(Self {
            file: spec.written.clone(),
            columns,
            rows: rows.into_iter().map(Arc::new).collect(),
            order: spec.order,
            exhausted: spec.exhausted,
            shuffled,
            taken: AtomicU64::new(0),
        })
    }

    /// How many rows there are.
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// The next iteration's row; `None` when they are used up under
    /// `stop`.
    pub(crate) fn take(&self, random: &mut Random) -> Option<Arc<Row>> {
        let n = self.taken.fetch_add(1, Ordering::Relaxed);
        let len = self.rows.len() as u64;
        let index = match (self.order, self.exhausted) {
            (_, DataExhausted::Stop) if n >= len => return None,
            (DataOrder::Sequential, _) => n % len,
            (DataOrder::Random, DataExhausted::Stop) => {
                self.shuffled[usize::try_from(n).ok()?] as u64
            }
            (DataOrder::Random, DataExhausted::Wrap) => {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                let i = (random.unit() * len as f64) as u64;
                i.min(len - 1)
            }
        };
        self.rows.get(usize::try_from(index).ok()?).cloned()
    }
}

/// A JSON array of objects: the columns are the first object's keys, then
/// any other's in order.
fn parse_json(text: &str) -> Result<(Vec<String>, Vec<Row>), String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let Value::Array(items) = value else {
        return Err("not a JSON array of objects, e.g. [{\"email\": \"a@example.com\"}]".into());
    };
    let mut columns: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let Value::Object(map) = item else {
            return Err(format!("item {i} is not an object"));
        };
        let mut row = Row::new();
        for (key, value) in map {
            if !columns.contains(&key) {
                columns.push(key.clone());
            }
            let text = match value {
                Value::String(s) => s,
                Value::Null => String::new(),
                other => other.to_string(),
            };
            row.insert(key, text);
        }
        rows.push(row);
    }
    // A row without a column has it empty, as a short CSV row cannot.
    for row in &mut rows {
        for column in &columns {
            row.entry(column.clone()).or_default();
        }
    }
    Ok((columns, rows))
}

/// A CSV file: a header line, then one line per row; fields may be
/// quoted (`"a, b"`, `""` for a quote); blank lines are left out.
fn parse_csv(text: &str) -> Result<(Vec<String>, Vec<Row>), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut records = csv_records(text)?.into_iter();
    let Some((_, header)) = records.next() else {
        return Err("the file is empty; its first line names the columns".into());
    };
    let columns: Vec<String> = header.iter().map(|c| c.trim().to_owned()).collect();
    if let Some(blank) = columns.iter().position(String::is_empty) {
        return Err(format!(
            "column {} has no name in the first line",
            blank + 1
        ));
    }
    for (i, c) in columns.iter().enumerate() {
        if columns[..i].contains(c) {
            return Err(format!("two columns are named {c}"));
        }
    }
    let mut rows = Vec::new();
    for (line, fields) in records {
        if fields.len() != columns.len() {
            return Err(format!(
                "line {line} has {} fields, and the header {}",
                fields.len(),
                columns.len()
            ));
        }
        rows.push(columns.iter().cloned().zip(fields).collect());
    }
    Ok((columns, rows))
}

/// Each record of `text` with the line it starts on.
fn csv_records(text: &str) -> Result<Vec<(usize, Vec<String>)>, String> {
    let mut records = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1;
    let mut start = 1;
    let mut chars = text.chars().peekable();
    let mut empty = true;
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' if quoted => quoted = false,
            '"' if field.is_empty() => {
                quoted = true;
                empty = false;
            }
            '\n' if quoted => {
                line += 1;
                field.push('\n');
            }
            ',' if !quoted => {
                fields.push(std::mem::take(&mut field));
                empty = false;
            }
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' => {
                if !empty || !field.is_empty() {
                    fields.push(std::mem::take(&mut field));
                    records.push((start, std::mem::take(&mut fields)));
                }
                empty = true;
                line += 1;
                start = line;
            }
            c => {
                field.push(c);
                empty = false;
            }
        }
    }
    if quoted {
        return Err(format!("a quote opened on line {start} is never closed"));
    }
    if !empty || !field.is_empty() {
        fields.push(field);
        records.push((start, fields));
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::profile::{DataExhausted, DataOrder, DataSpec};
    use super::super::user::Random;
    use super::{Data, parse_csv, parse_json};

    fn spec(path: &Path, order: DataOrder, exhausted: DataExhausted) -> DataSpec {
        DataSpec {
            written: "users.csv".into(),
            path: path.to_owned(),
            order,
            exhausted,
            at: protocol::script::Span {
                file: "p.toml".into(),
                line: 1,
                column: 1,
                start: 0,
                end: 0,
            },
        }
    }

    #[test]
    fn csv_has_a_header_and_quotes() {
        let (columns, rows) =
            parse_csv("email,name\r\na@x.com,\"Ann, A\"\n\nb@x.com,\"say \"\"hi\"\"\"\n").unwrap();
        assert_eq!(columns, ["email", "name"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Ann, A");
        assert_eq!(rows[1]["name"], "say \"hi\"");
        assert_eq!(
            parse_csv("a,b\n1\n").unwrap_err(),
            "line 2 has 1 fields, and the header 2"
        );
        assert_eq!(
            parse_csv("a,a\n1,2\n").unwrap_err(),
            "two columns are named a"
        );
        assert!(parse_csv("a\n\"1\n").unwrap_err().contains("never closed"));
        let (_, rows) = parse_csv("a,b\n\"x\ny\",2").unwrap();
        assert_eq!(rows[0]["a"], "x\ny");
    }

    #[test]
    fn json_is_an_array_of_objects() {
        let (columns, rows) =
            parse_json(r#"[{"email": "a@x.com", "id": 3}, {"email": "b@x.com", "vip": true}]"#)
                .unwrap();
        assert_eq!(columns, ["email", "id", "vip"]);
        assert_eq!(rows[0]["id"], "3");
        assert_eq!(rows[0]["vip"], "");
        assert_eq!(rows[1]["vip"], "true");
        assert!(parse_json("{}").is_err());
        assert!(parse_json("[1]").is_err());
    }

    fn feeder(order: DataOrder, exhausted: DataExhausted) -> (tempfile::TempDir, Data) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.csv");
        std::fs::write(&path, "n\n0\n1\n2\n3\n").unwrap();
        let data = Data::load(&spec(&path, order, exhausted), 5).unwrap();
        (dir, data)
    }

    fn take(data: &Data, n: usize) -> Vec<Option<String>> {
        let mut random = Random::new(1);
        (0..n)
            .map(|_| data.take(&mut random).map(|row| row["n"].clone()))
            .collect()
    }

    #[test]
    fn rows_are_taken_in_order_or_at_random_and_wrap_or_stop() {
        let some = |v: &[&str]| v.iter().map(|s| Some((*s).to_owned())).collect::<Vec<_>>();
        let (_d, data) = feeder(DataOrder::Sequential, DataExhausted::Wrap);
        assert_eq!(take(&data, 6), some(&["0", "1", "2", "3", "0", "1"]));
        let (_d, data) = feeder(DataOrder::Sequential, DataExhausted::Stop);
        let mut taken = take(&data, 5);
        assert_eq!(taken.pop(), Some(None));
        assert_eq!(taken, some(&["0", "1", "2", "3"]));
        // Random and stop: each row once, in some order.
        let (_d, data) = feeder(DataOrder::Random, DataExhausted::Stop);
        let mut taken = take(&data, 5);
        assert_eq!(taken.pop(), Some(None));
        taken.sort();
        assert_eq!(taken, some(&["0", "1", "2", "3"]));
        // Random and wrap: rows spread evenly, without end.
        let (_d, data) = feeder(DataOrder::Random, DataExhausted::Wrap);
        let taken = take(&data, 4000);
        for n in ["0", "1", "2", "3"] {
            let count = taken.iter().filter(|t| t.as_deref() == Some(n)).count();
            assert!((900..1100).contains(&count), "{n}: {count}");
        }
    }

    #[test]
    fn a_file_without_rows_is_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.csv");
        std::fs::write(&path, "email\n").unwrap();
        let e =
            Data::load(&spec(&path, DataOrder::Sequential, DataExhausted::Wrap), 0).unwrap_err();
        assert_eq!(e, "data users.csv has no rows");
        let e = Data::load(
            &spec(
                &dir.path().join("nope.csv"),
                DataOrder::Sequential,
                DataExhausted::Wrap,
            ),
            0,
        )
        .unwrap_err();
        assert!(e.starts_with("cannot read data users.csv"), "{e}");
    }
}
