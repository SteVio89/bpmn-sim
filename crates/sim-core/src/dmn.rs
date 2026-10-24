//! DMN 1.3 decision tables (spec §5.9): reading, checking and evaluating them with the hit
//! policies UNIQUE, FIRST, ANY and COLLECT (optionally with SUM, COUNT, MIN or MAX).

use std::collections::BTreeMap;

use roxmltree::Node;

use crate::error::ParseError;
use crate::feel::{Op, Test, UnaryTests, Value, parse_output};

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
    /// `<description>` of the decision.
    pub description: Option<String>,
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
    /// What the file says about each input (same order as `inputs`; may be shorter).
    pub input_info: Vec<InputInfo>,
    /// Output names (empty string for an unnamed single output).
    pub outputs: Vec<String>,
    pub rules: Vec<RawRule>,
}

/// An input's `label`, its expression's `typeRef` and its `inputValues` (allowed values).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputInfo {
    pub label: Option<String>,
    pub type_ref: Option<String>,
    pub allowed: Option<String>,
}

/// What kind of value a decision input takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Text,
    Number,
    Bool,
    /// No `typeRef` and no rule literal to tell.
    Unknown,
}

/// A decision input as a person filling in a scenario needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct InputDescription {
    /// The variable name (input expression).
    pub name: String,
    pub label: Option<String>,
    pub kind: InputKind,
    /// Text values to choose from: the allowed values (`inputValues`), else those the rules
    /// compare with, then any further ones the rules compare with.
    pub values: Vec<String>,
    /// Numbers the rules compare with (range bounds included), ascending.
    pub thresholds: Vec<f64>,
    /// Whether a rule tests `not(...)`: some value other than the listed ones matters.
    pub negated: bool,
}

impl InputDescription {
    /// "Issue type (issueType)", or just the name without a label.
    pub fn title(&self) -> String {
        match &self.label {
            Some(l) if *l != self.name => format!("{l} ({})", self.name),
            _ => self.name.clone(),
        }
    }
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
            input_info: children(t, "input")
                .map(|i| InputInfo {
                    label: i.attribute("label").map(str::to_string),
                    type_ref: children(i, "inputExpression")
                        .next()
                        .and_then(|e| e.attribute("typeRef"))
                        .map(str::to_string),
                    allowed: children(i, "inputValues")
                        .next()
                        .map(text_of)
                        .filter(|t| !t.is_empty()),
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
                description: children(d, "description")
                    .next()
                    .and_then(|t| t.text())
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
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
    pub input_info: Vec<InputInfo>,
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
            input_info: raw.input_info.clone(),
            outputs: raw.outputs.clone(),
            rules,
        })
    }

    /// The values output `output` can take, in rule order and without repeats; a null entry
    /// is left out (a rule that gives null is like no match). `None` when an aggregation
    /// computes the value. Without aggregation COLLECT gives a list of these values.
    pub fn values(&self, output: &str) -> Option<Vec<Value>> {
        let i = self.outputs.iter().position(|o| o == output)?;
        if matches!(self.hit_policy, HitPolicy::Collect(Some(_))) {
            return None;
        }
        let mut out: Vec<Value> = Vec::new();
        for (_, outs) in &self.rules {
            if outs[i] != Value::Null && !out.contains(&outs[i]) {
                out.push(outs[i].clone());
            }
        }
        Some(out)
    }

    /// The literals the rules compare input `input` with (range bounds included), in rule
    /// order and without repeats, and whether a rule negates its tests (`not(...)`).
    pub fn literals(&self, input: &str) -> (Vec<Value>, bool) {
        let Some(i) = self.inputs.iter().position(|x| x == input) else {
            return (Vec::new(), false);
        };
        let negated = self
            .rules
            .iter()
            .any(|(tests, _)| matches!(tests[i], UnaryTests::Tests { negated: true, .. }));
        let mut out: Vec<Value> = Vec::new();
        let mut add = |v: &Value| {
            if *v != Value::Null && !out.contains(v) {
                out.push(v.clone());
            }
        };
        for (tests, _) in &self.rules {
            if let UnaryTests::Tests { tests, .. } = &tests[i] {
                for t in tests {
                    match t {
                        Test::Cmp(_, v) => add(v),
                        Test::Range { lo, hi, .. } => {
                            add(lo);
                            add(hi);
                        }
                    }
                }
            }
        }
        (out, negated)
    }

    /// Input `name` described for filling in a scenario: its label, kind (from `typeRef`, else
    /// from the literals the rules use), the text values to choose from and the numbers the
    /// rules compare with.
    pub fn describe_input(&self, name: &str) -> Option<InputDescription> {
        let i = self.inputs.iter().position(|x| x == name)?;
        let info = self.input_info.get(i).cloned().unwrap_or_default();
        let (literals, negated) = self.literals(name);
        let mut values: Vec<String> = Vec::new();
        let allowed = info
            .allowed
            .as_deref()
            .and_then(|t| UnaryTests::parse(t).ok());
        let from_allowed = allowed.iter().flat_map(|u| match u {
            UnaryTests::Tests {
                negated: false,
                tests,
            } => tests.clone(),
            _ => Vec::new(),
        });
        for t in from_allowed {
            if let Test::Cmp(Op::Eq, Value::Str(v)) = t
                && !values.contains(&v)
            {
                values.push(v);
            }
        }
        for v in &literals {
            if let Value::Str(v) = v
                && !values.contains(v)
            {
                values.push(v.clone());
            }
        }
        let mut thresholds: Vec<f64> = literals
            .iter()
            .filter_map(|v| match v {
                Value::Num(x) => Some(*x),
                _ => None,
            })
            .collect();
        thresholds.sort_by(f64::total_cmp);
        thresholds.dedup();
        let kind = match info
            .type_ref
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("string") => InputKind::Text,
            Some("number" | "integer" | "long" | "double") => InputKind::Number,
            Some("boolean") => InputKind::Bool,
            _ if !values.is_empty() => InputKind::Text,
            _ if literals.iter().any(|v| matches!(v, Value::Bool(_))) => InputKind::Bool,
            _ if !thresholds.is_empty() => InputKind::Number,
            _ => InputKind::Unknown,
        };
        Some(InputDescription {
            name: name.to_string(),
            label: info.label.filter(|l| !l.trim().is_empty()),
            kind,
            values,
            thresholds,
            negated,
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
            input_info: Vec::new(),
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
    fn inputs_are_described_from_the_file_and_the_rules() {
        let xml = r#"<definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/" id="D">
  <decision id="d" name="Routing">
    <description> Picks a route. </description>
    <decisionTable id="t">
      <input id="i1" label="Kind of item"><inputExpression id="e1" typeRef="string"><text>kind</text></inputExpression><inputValues id="v1"><text>"small","large","bulky"</text></inputValues></input>
      <input id="i2" label="Weight"><inputExpression id="e2" typeRef="number"><text>weight</text></inputExpression></input>
      <input id="i3"><inputExpression id="e3"><text>fragile</text></inputExpression></input>
      <output id="o" name="route"/>
      <rule id="r1"><inputEntry id="a"><text>"large","odd"</text></inputEntry><inputEntry id="b"><text>[10..20]</text></inputEntry><inputEntry id="c"><text>true</text></inputEntry><outputEntry id="d"><text>"truck"</text></outputEntry></rule>
      <rule id="r2"><inputEntry id="a2"><text>not("small")</text></inputEntry><inputEntry id="b2"><text>&lt; 5</text></inputEntry><inputEntry id="c2"><text>-</text></inputEntry><outputEntry id="d2"><text>"van"</text></outputEntry></rule>
    </decisionTable>
  </decision>
</definitions>"#;
        let f = parse_dmn("r.dmn", xml).unwrap();
        let d = &f.decisions["d"];
        assert_eq!(d.description.as_deref(), Some("Picks a route."));
        let t = Table::compile("d", d.table.as_ref().unwrap()).unwrap();
        let kind = t.describe_input("kind").unwrap();
        assert_eq!(kind.title(), "Kind of item (kind)");
        assert_eq!(kind.kind, InputKind::Text);
        assert_eq!(kind.values, ["small", "large", "bulky", "odd"]);
        assert!(kind.negated);
        let weight = t.describe_input("weight").unwrap();
        assert_eq!(
            (weight.kind, weight.thresholds.clone()),
            (InputKind::Number, vec![5.0, 10.0, 20.0])
        );
        let fragile = t.describe_input("fragile").unwrap();
        assert_eq!(
            (fragile.kind, fragile.title()),
            (InputKind::Bool, "fragile".into())
        );
        assert!(t.describe_input("nothing").is_none());
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
            input_info: Vec::new(),
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
