//! DMN 1.3 decision tables (spec §5.9): reading, checking and evaluating them with the hit
//! policies UNIQUE, FIRST, ANY and COLLECT (optionally with SUM, COUNT, MIN or MAX).

use std::collections::BTreeMap;

use roxmltree::Node;

use crate::error::ParseError;
use crate::feel::{UnaryTests, Value, parse_output};

#[derive(Debug, Clone, Default)]
pub struct DmnFile {
    pub name: String,
    /// Decision id -> decision.
    pub decisions: BTreeMap<String, Decision>,
}

#[derive(Debug, Clone, Default)]
pub struct Decision {
    pub id: String,
    pub name: Option<String>,
    /// `None` if the decision is not a decision table (e.g. a literal expression).
    pub table: Option<RawTable>,
}

/// A decision table as written; entries are compiled when a scenario uses the decision.
#[derive(Debug, Clone, Default)]
pub struct RawTable {
    pub hit_policy: String,
    pub aggregation: Option<String>,
    /// Input expressions (variable names).
    pub inputs: Vec<String>,
    /// Output names (empty string for an unnamed single output).
    pub outputs: Vec<String>,
    pub rules: Vec<RawRule>,
}

#[derive(Debug, Clone, Default)]
pub struct RawRule {
    pub id: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: Node) -> String {
    children(n, "text")
        .next()
        .and_then(|t| t.text())
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn parse_dmn(file: &str, xml: &str) -> Result<DmnFile, ParseError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| ParseError::Xml {
        file: file.to_string(),
        message: e.to_string(),
    })?;
    let root = doc.root_element();
    let is_dmn = root.tag_name().name() == "definitions"
        && root
            .tag_name()
            .namespace()
            .is_some_and(|n| n.contains("/DMN/"));
    if !is_dmn {
        return Err(ParseError::NotDmn {
            file: file.to_string(),
        });
    }
    let mut out = DmnFile {
        name: file.to_string(),
        ..DmnFile::default()
    };
    for d in children(root, "decision") {
        let Some(id) = d.attribute("id") else {
            return Err(ParseError::Element {
                file: file.to_string(),
                element: "<decision>".into(),
                message: "element has no 'id' attribute".into(),
            });
        };
        let table = children(d, "decisionTable").next().map(|t| RawTable {
            hit_policy: t.attribute("hitPolicy").unwrap_or("UNIQUE").to_string(),
            aggregation: t.attribute("aggregation").map(str::to_string),
            inputs: children(t, "input")
                .map(|i| {
                    children(i, "inputExpression")
                        .next()
                        .map(text_of)
                        .unwrap_or_default()
                })
                .collect(),
            outputs: children(t, "output")
                .map(|o| o.attribute("name").unwrap_or_default().to_string())
                .collect(),
            rules: children(t, "rule")
                .map(|r| RawRule {
                    id: r.attribute("id").unwrap_or_default().to_string(),
                    inputs: children(r, "inputEntry").map(text_of).collect(),
                    outputs: children(r, "outputEntry").map(text_of).collect(),
                })
                .collect(),
        });
        out.decisions.insert(
            id.to_string(),
            Decision {
                id: id.to_string(),
                name: d.attribute("name").map(str::to_string),
                table,
            },
        );
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitPolicy {
    Unique,
    First,
    Any,
    Collect(Option<Aggregation>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregation {
    Sum,
    Count,
    Min,
    Max,
}

/// A decision table ready to evaluate.
#[derive(Debug, Clone)]
pub struct Table {
    pub hit_policy: HitPolicy,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    rules: Vec<(Vec<UnaryTests>, Vec<Value>)>,
}

/// Result of one evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// Output name -> value. For COLLECT without aggregation the values are lists.
    pub outputs: BTreeMap<String, Value>,
    pub matched: usize,
    /// UNIQUE with several matches, or ANY with different outputs (the first match is used).
    pub violation: bool,
}

impl Table {
    pub fn compile(decision: &str, raw: &RawTable) -> Result<Self, String> {
        let err = |m: String| format!("decision '{decision}': {m}");
        let hit_policy = match raw.hit_policy.to_ascii_uppercase().as_str() {
            "UNIQUE" => HitPolicy::Unique,
            "FIRST" => HitPolicy::First,
            "ANY" => HitPolicy::Any,
            "COLLECT" => HitPolicy::Collect(
                match raw
                    .aggregation
                    .as_deref()
                    .map(str::to_ascii_uppercase)
                    .as_deref()
                {
                    None => None,
                    Some("SUM") => Some(Aggregation::Sum),
                    Some("COUNT") => Some(Aggregation::Count),
                    Some("MIN") => Some(Aggregation::Min),
                    Some("MAX") => Some(Aggregation::Max),
                    Some(a) => return Err(err(format!("unknown COLLECT aggregation '{a}'"))),
                },
            ),
            other => {
                return Err(err(format!(
                    "hit policy {other} is not supported (UNIQUE, FIRST, ANY, COLLECT)"
                )));
            }
        };
        for i in &raw.inputs {
            if i.is_empty()
                || !i
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                return Err(err(format!(
                    "input expression '{i}' must be a variable name"
                )));
            }
        }
        if raw.outputs.is_empty() {
            return Err(err("decision table has no output".into()));
        }
        let mut rules = Vec::new();
        for r in &raw.rules {
            if r.inputs.len() != raw.inputs.len() || r.outputs.len() != raw.outputs.len() {
                return Err(err(format!(
                    "rule '{}' does not have one entry per input and output",
                    r.id
                )));
            }
            let tests = r
                .inputs
                .iter()
                .map(|t| UnaryTests::parse(t))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| err(format!("rule '{}': {e}", r.id)))?;
            let outs = r
                .outputs
                .iter()
                .map(|o| parse_output(o))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| err(format!("rule '{}': {e}", r.id)))?;
            rules.push((tests, outs));
        }
        Ok(Self {
            hit_policy,
            inputs: raw.inputs.clone(),
            outputs: raw.outputs.clone(),
            rules,
        })
    }

    /// `input(name)` gives the value of an input expression.
    pub fn evaluate(&self, mut input: impl FnMut(&str) -> Value) -> Evaluation {
        let values: Vec<Value> = self.inputs.iter().map(|i| input(i)).collect();
        let hits: Vec<&Vec<Value>> = self
            .rules
            .iter()
            .filter(|(tests, _)| tests.iter().zip(&values).all(|(t, v)| t.matches(v)))
            .map(|(_, outs)| outs)
            .collect();
        let named = |outs: &[Value]| -> BTreeMap<String, Value> {
            self.outputs
                .iter()
                .cloned()
                .zip(outs.iter().cloned())
                .collect()
        };
        let nulls = || -> BTreeMap<String, Value> {
            self.outputs
                .iter()
                .map(|o| (o.clone(), Value::Null))
                .collect()
        };
        let (outputs, violation) = match self.hit_policy {
            HitPolicy::Unique | HitPolicy::First | HitPolicy::Any => match hits.first() {
                None => (nulls(), false),
                Some(first) => {
                    let violation = match self.hit_policy {
                        HitPolicy::Unique => hits.len() > 1,
                        HitPolicy::Any => hits.iter().any(|h| h != first),
                        _ => false,
                    };
                    (named(first), violation)
                }
            },
            HitPolicy::Collect(agg) => {
                let column = |i: usize| hits.iter().map(|h| h[i].clone()).collect::<Vec<Value>>();
                let outputs = self
                    .outputs
                    .iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let col = column(i);
                        let nums = || {
                            col.iter().filter_map(|v| {
                                if let Value::Num(n) = v {
                                    Some(*n)
                                } else {
                                    None
                                }
                            })
                        };
                        let v = match agg {
                            None => Value::List(col.clone()),
                            Some(Aggregation::Count) => Value::Num(col.len() as f64),
                            Some(Aggregation::Sum) => Value::Num(nums().sum()),
                            Some(Aggregation::Min) => {
                                nums().reduce(f64::min).map_or(Value::Null, Value::Num)
                            }
                            Some(Aggregation::Max) => {
                                nums().reduce(f64::max).map_or(Value::Null, Value::Num)
                            }
                        };
                        (name.clone(), v)
                    })
                    .collect();
                (outputs, false)
            }
        };
        Evaluation {
            outputs,
            matched: hits.len(),
            violation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(policy: &str, agg: Option<&str>, rules: &[(&[&str], &[&str])]) -> Table {
        let raw = RawTable {
            hit_policy: policy.into(),
            aggregation: agg.map(String::from),
            inputs: vec!["amount".into(), "segment".into()],
            outputs: vec!["result".into()],
            rules: rules
                .iter()
                .enumerate()
                .map(|(i, (ins, outs))| RawRule {
                    id: format!("r{i}"),
                    inputs: ins.iter().map(|s| s.to_string()).collect(),
                    outputs: outs.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
        };
        Table::compile("d", &raw).unwrap()
    }

    fn eval(t: &Table, amount: f64, segment: &str) -> Evaluation {
        t.evaluate(|name| match name {
            "amount" => Value::Num(amount),
            _ => Value::Str(segment.into()),
        })
    }

    const RULES: &[(&[&str], &[&str])] = &[
        (&["< 1000", "-"], &["\"auto\""]),
        (&["[1000..5000]", "\"gold\""], &["\"manual\""]),
        (&["[1000..5000]", "not(\"gold\")"], &["\"auto\""]),
        (&["> 5000", "-"], &["\"reject\""]),
    ];

    #[test]
    fn unique_and_first() {
        let t = table("UNIQUE", None, RULES);
        assert_eq!(
            eval(&t, 10.0, "gold").outputs["result"],
            Value::Str("auto".into())
        );
        assert_eq!(
            eval(&t, 2000.0, "gold").outputs["result"],
            Value::Str("manual".into())
        );
        assert_eq!(
            eval(&t, 5000.0, "silver").outputs["result"],
            Value::Str("auto".into())
        );
        assert_eq!(
            eval(&t, 9000.0, "x").outputs["result"],
            Value::Str("reject".into())
        );
        assert!(!eval(&t, 9000.0, "x").violation);

        // Overlapping rules: UNIQUE reports a violation, FIRST takes the first.
        let overlap: &[(&[&str], &[&str])] = &[(&["< 10", "-"], &["1"]), (&["< 20", "-"], &["2"])];
        let u = eval(&table("UNIQUE", None, overlap), 5.0, "");
        assert!(u.violation && u.matched == 2);
        let f = eval(&table("FIRST", None, overlap), 5.0, "");
        assert_eq!(
            (f.outputs["result"].clone(), f.violation),
            (Value::Num(1.0), false)
        );
        // No match: null.
        let none = eval(&table("FIRST", None, overlap), 50.0, "");
        assert_eq!(
            (none.outputs["result"].clone(), none.matched),
            (Value::Null, 0)
        );
    }

    #[test]
    fn any_requires_equal_outputs() {
        let same: &[(&[&str], &[&str])] = &[(&["< 10", "-"], &["1"]), (&["< 20", "-"], &["1"])];
        assert!(!eval(&table("ANY", None, same), 5.0, "").violation);
        let different: &[(&[&str], &[&str])] =
            &[(&["< 10", "-"], &["1"]), (&["< 20", "-"], &["2"])];
        assert!(eval(&table("ANY", None, different), 5.0, "").violation);
    }

    #[test]
    fn collect_with_and_without_aggregation() {
        let rules: &[(&[&str], &[&str])] = &[
            (&["> 0", "-"], &["10"]),
            (&["> 5", "-"], &["20"]),
            (&["> 100", "-"], &["40"]),
        ];
        let at = |agg: Option<&str>| {
            eval(&table("COLLECT", agg, rules), 50.0, "").outputs["result"].clone()
        };
        assert_eq!(
            at(None),
            Value::List(vec![Value::Num(10.0), Value::Num(20.0)])
        );
        assert_eq!(at(Some("SUM")), Value::Num(30.0));
        assert_eq!(at(Some("COUNT")), Value::Num(2.0));
        assert_eq!(at(Some("MIN")), Value::Num(10.0));
        assert_eq!(at(Some("MAX")), Value::Num(20.0));
    }

    #[test]
    fn unsupported_tables_are_rejected() {
        let raw = |policy: &str, input: &str, entry: &str| RawTable {
            hit_policy: policy.into(),
            aggregation: None,
            inputs: vec![input.into()],
            outputs: vec!["o".into()],
            rules: vec![RawRule {
                id: "r".into(),
                inputs: vec![entry.into()],
                outputs: vec!["1".into()],
            }],
        };
        assert!(
            Table::compile("d", &raw("PRIORITY", "x", "-"))
                .unwrap_err()
                .contains("PRIORITY")
        );
        assert!(
            Table::compile("d", &raw("UNIQUE", "x + 1", "-"))
                .unwrap_err()
                .contains("variable name")
        );
        assert!(
            Table::compile("d", &raw("UNIQUE", "x", "date(\"2020-01-01\")"))
                .unwrap_err()
                .contains("rule 'r'")
        );
    }
}
