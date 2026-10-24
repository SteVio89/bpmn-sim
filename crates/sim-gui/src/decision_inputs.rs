//! Editors for the inputs of DMN decisions: each input under its DMN label, edited as what it
//! is (one of several values, yes/no, a number), and a preview of what the decision gives.

use eframe::egui::{self, ComboBox, Id, RichText, Ui};
use sim_core::Project;
use sim_core::Scenario;
use sim_core::dmn::{InputDescription, InputKind};
use sim_core::needs::{DecisionPreview, describe_input, preview_decision, threshold_shares};
use sim_core::scenario::{ChanceSpec, ChoiceSpec, InputSpec, NumberSpec};

use crate::diagram_panel::case_color;
use crate::editors::number_fields;

/// How many cases the previews draw.
const SAMPLES: usize = 2000;

/// The inputs of decision `decision` that the scenario sets, with the decision's name and
/// description above and what it gives below.
pub fn decision_editor(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    decision: &str,
    inputs: &[String],
) {
    let d = find_decision(project, decision);
    ui.label(RichText::new(format!("Decision {}", decision_name(project, decision))).strong())
        .on_hover_text(format!("decision id {decision}"));
    if let Some(text) = d.and_then(|d| d.description.as_deref()) {
        ui.label(RichText::new(text).small().weak());
    }
    ui.label(
        RichText::new("How often each input value occurs among the cases:")
            .small()
            .weak(),
    );
    for i in inputs {
        ui.add_space(4.0);
        match s.decision_input_mut(i) {
            Some((owner, spec)) => {
                let owner = owner.clone();
                input_edit(ui, project, decision, i, spec);
                if owner != decision {
                    ui.label(
                        RichText::new(format!(
                            "Shared with decision {}: one value per case serves every decision that reads {i}.",
                            decision_name(project, &owner)
                        ))
                        .small()
                        .weak(),
                    );
                }
            }
            None => {
                ui.label(format!("{i}: not set"));
            }
        }
    }
    ui.add_space(6.0);
    preview(ui, project, s, decision);
}

fn find_decision<'p>(project: &'p Project, id: &str) -> Option<&'p sim_core::dmn::Decision> {
    project
        .decisions
        .get(id)
        .and_then(|f| project.dmn.get(f))
        .and_then(|f| f.decisions.get(id))
}

/// The decision's name in quotes, else its id.
fn decision_name(project: &Project, id: &str) -> String {
    match find_decision(project, id).and_then(|d| d.name.as_deref()) {
        Some(n) => format!("\u{201c}{n}\u{201d}"),
        None => id.to_string(),
    }
}

/// One decision input: its label and type, then an editor fitting its kind.
pub fn input_edit(
    ui: &mut Ui,
    project: &Project,
    decision: &str,
    input: &str,
    spec: &mut InputSpec,
) {
    let desc = describe_input(project, decision, input);
    let id = Id::unique(("decision-input", decision, input));
    let kind = desc.as_ref().map_or(InputKind::Unknown, |d| d.kind);
    ui.horizontal(|ui| {
        let title = desc
            .as_ref()
            .and_then(|d| d.label.clone())
            .unwrap_or(input.to_string());
        ui.label(RichText::new(title).strong());
        let type_text = match kind {
            InputKind::Text => "text",
            InputKind::Number => "number",
            InputKind::Bool => "yes/no",
            InputKind::Unknown => "type not given",
        };
        ui.label(
            RichText::new(format!("{input} · {type_text}"))
                .small()
                .weak(),
        )
        .on_hover_text("variable name and type in the decision table");
        // The kind follows the DMN type; it is only offered when that does not say or does
        // not fit what the scenario has.
        let fits = matches!(
            (kind, &*spec),
            (InputKind::Text, InputSpec::Choice(_))
                | (InputKind::Bool, InputSpec::Chance(_))
                | (InputKind::Number, InputSpec::Number(_))
        );
        if !fits {
            kind_combo(ui, id, spec, desc.as_ref());
        }
    });
    ui.indent(id.with("body"), |ui| match spec {
        InputSpec::Choice(c) => choice_edit(ui, id, c, desc.as_ref()),
        InputSpec::Chance(c) => chance_edit(ui, c),
        InputSpec::Number(n) => number_edit(ui, id, n, desc.as_ref()),
    });
}

const KINDS: [&str; 3] = ["one of several values", "yes / no", "a number"];

fn kind_combo(ui: &mut Ui, id: Id, spec: &mut InputSpec, desc: Option<&InputDescription>) {
    let kind = match spec {
        InputSpec::Choice(_) => 0,
        InputSpec::Chance(_) => 1,
        InputSpec::Number(_) => 2,
    };
    let mut chosen = kind;
    ComboBox::from_id_salt(id.with("kind"))
        .selected_text(KINDS[kind])
        .show_ui(ui, |ui| {
            for (i, k) in KINDS.iter().enumerate() {
                ui.selectable_value(&mut chosen, i, *k);
            }
        });
    if chosen != kind {
        *spec = match chosen {
            0 => {
                let mut values = desc.map(|d| d.values.clone()).unwrap_or_default();
                if values.is_empty() {
                    values.push("value".into());
                }
                let p = 1.0 / values.len() as f64;
                InputSpec::Choice(ChoiceSpec {
                    choice: values.into_iter().map(|v| (v, p)).collect(),
                })
            }
            1 => InputSpec::Chance(ChanceSpec { p_true: 0.5 }),
            _ => InputSpec::Number(NumberSpec::Uniform {
                min: 0.0,
                max: 100.0,
            }),
        };
    }
}

/// One of several values: a share per value, values can be added and removed.
fn choice_edit(ui: &mut Ui, id: Id, c: &mut ChoiceSpec, desc: Option<&InputDescription>) {
    let known = |v: &str| desc.is_none_or(|d| d.values.iter().any(|x| x == v));
    let mut remove = None;
    egui::Grid::new(id.with("choice"))
        .num_columns(3)
        .show(ui, |ui| {
            let several = c.choice.len() > 1;
            for (i, (value, p)) in c.choice.iter_mut().enumerate() {
                let label = ui.colored_label(case_color(i as u64), value.as_str());
                if !known(value) {
                    label.on_hover_text(
                        "No rule names this value: it only matches rules with \"-\" or not(...).",
                    );
                }
                let mut pct = *p * 100.0;
                if ui
                    .add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix(" %"))
                    .changed()
                {
                    *p = pct / 100.0;
                }
                if several
                    && ui
                        .small_button("×")
                        .on_hover_text("remove this value")
                        .clicked()
                {
                    remove = Some(value.clone());
                }
                ui.end_row();
            }
        });
    if let Some(v) = remove {
        c.choice.remove(&v);
    }
    // Values the decision knows that are not listed yet, and a free one.
    let missing: Vec<&String> = desc
        .map(|d| {
            d.values
                .iter()
                .filter(|v| !c.choice.contains_key(*v))
                .collect()
        })
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        for v in missing {
            if ui
                .small_button(format!("+ {v}"))
                .on_hover_text("add this value with 0 %")
                .clicked()
            {
                c.choice.insert(v.clone(), 0.0);
            }
        }
        let text_id = id.with("new-value");
        let mut text = ui.data_mut(|d| d.get_temp::<String>(text_id).unwrap_or_default());
        let edit = ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text("other value")
                .desired_width(110.0),
        );
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let name = text.trim().trim_matches('"').to_string();
        if (ui.small_button("Add").clicked() || enter)
            && !name.is_empty()
            && !c.choice.contains_key(&name)
        {
            c.choice.insert(name, 0.0);
            text.clear();
        }
        ui.data_mut(|d| d.insert_temp(text_id, text));
    });
    let sum: f64 = c.choice.values().sum();
    if (sum - 1.0).abs() > 1e-9 {
        ui.horizontal(|ui| {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("shares add up to {:.1} %", sum * 100.0),
            );
            if sum > 0.0 && ui.button("Scale to 100 %").clicked() {
                c.choice.values_mut().for_each(|p| *p /= sum);
            }
        });
    }
    if desc.is_some_and(|d| d.negated) {
        ui.label(
            RichText::new(
                "A rule tests not(...): a value other than the ones it names takes that rule.",
            )
            .small()
            .weak(),
        );
    }
}

/// Yes/no: how often it is true.
fn chance_edit(ui: &mut Ui, c: &mut ChanceSpec) {
    ui.horizontal(|ui| {
        ui.label("true in");
        let mut pct = c.p_true * 100.0;
        if ui
            .add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix(" %"))
            .changed()
        {
            c.p_true = pct / 100.0;
        }
        ui.label(format!(
            "of the cases, false in {:.0} %",
            (1.0 - c.p_true) * 100.0
        ));
    });
}

const DISTS: [&str; 6] = [
    "fixed",
    "uniform",
    "normal",
    "exponential",
    "triangular",
    "lognormal",
];

/// A number: its distribution, and how often it falls on each side of the numbers the rules
/// compare with.
fn number_edit(ui: &mut Ui, id: Id, n: &mut NumberSpec, desc: Option<&InputDescription>) {
    ui.horizontal(|ui| {
        let k = match n {
            NumberSpec::Fixed { .. } => 0,
            NumberSpec::Uniform { .. } => 1,
            NumberSpec::Normal { .. } => 2,
            NumberSpec::Exponential { .. } => 3,
            NumberSpec::Triangular { .. } => 4,
            NumberSpec::Lognormal { .. } => 5,
        };
        let mut c = k;
        ComboBox::from_id_salt(id.with("dist"))
            .width(90.0)
            .selected_text(DISTS[k])
            .show_ui(ui, |ui| {
                for (i, name) in DISTS.iter().enumerate() {
                    ui.selectable_value(&mut c, i, *name);
                }
            });
        if c != k {
            *n = number_default(c, desc.map_or(&[][..], |d| &d.thresholds));
        }
        number_fields(ui, n);
    });
    let Some(d) = desc.filter(|d| !d.thresholds.is_empty()) else {
        return;
    };
    let t = &d.thresholds;
    let Some(bins) = threshold_shares(n, t, SAMPLES) else {
        return;
    };
    let pct = |share: f64| format!("{:.0} %", share * 100.0);
    let num = |x: f64| format!("{x}");
    let mut parts = vec![format!("below {}: {}", num(t[0]), pct(bins[0]))];
    for (i, w) in t.windows(2).enumerate() {
        parts.push(format!(
            "{} to {}: {}",
            num(w[0]),
            num(w[1]),
            pct(bins[i + 1])
        ));
    }
    parts.push(format!(
        "{} or more: {}",
        num(t[t.len() - 1]),
        pct(bins[t.len()])
    ));
    ui.label(
        RichText::new(format!("The rules compare with {}.", list(t)))
            .small()
            .weak(),
    );
    ui.label(RichText::new(parts.join(" · ")).small());
}

/// A distribution of kind `k` around the compared numbers (0..100 without any).
fn number_default(k: usize, thresholds: &[f64]) -> NumberSpec {
    let (lo, hi) = match (thresholds.first(), thresholds.last()) {
        (Some(&lo), Some(&hi)) if hi > lo => (lo, hi),
        (Some(&x), _) => ((x - x.abs().max(1.0)).max(0.0), x + x.abs().max(1.0)),
        _ => (0.0, 100.0),
    };
    let mid = (lo + hi) / 2.0;
    match k {
        0 => NumberSpec::Fixed { value: mid },
        1 => NumberSpec::Uniform { min: lo, max: hi },
        2 => NumberSpec::Normal {
            mean: mid,
            stdev: (hi - lo) / 4.0,
        },
        3 => NumberSpec::Exponential { mean: mid },
        4 => NumberSpec::Triangular {
            min: lo,
            mode: mid,
            max: hi,
        },
        _ => NumberSpec::Lognormal {
            mean: mid,
            stdev: (hi - lo) / 4.0,
        },
    }
}

/// "3", "3 and 5", "1, 3 and 5".
fn list(xs: &[f64]) -> String {
    let t: Vec<String> = xs.iter().map(|x| x.to_string()).collect();
    match t.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the decision gives with the scenario's inputs, from `SAMPLES` drawn cases. Kept until
/// the inputs change, so it is not recomputed every frame.
fn preview(ui: &mut Ui, project: &Project, s: &Scenario, decision: &str) {
    let key = Id::unique(("decision-preview", decision, format!("{:?}", s.decisions)));
    let p: Option<DecisionPreview> = ui.data_mut(|d| d.get_temp(key)).or_else(|| {
        let p = preview_decision(project, s, decision, SAMPLES);
        ui.data_mut(|d| d.insert_temp(key, p.clone()));
        p
    });
    let Some(p) = p else {
        return;
    };
    ui.label(RichText::new("What it gives").strong())
        .on_hover_text(format!(
            "The decision evaluated for {} cases drawn from these inputs.",
            p.samples
        ));
    let share = |c: usize| c as f64 * 100.0 / p.samples as f64;
    egui::Grid::new(("preview", decision))
        .num_columns(2)
        .show(ui, |ui| {
            for (output, values) in &p.outputs {
                ui.label(if output.is_empty() { "result" } else { output });
                ui.horizontal_wrapped(|ui| {
                    for (i, (v, c)) in values.iter().enumerate() {
                        let text = format!("{v} {:.0} %", share(*c));
                        if v == "nothing" {
                            ui.colored_label(ui.visuals().warn_fg_color, text);
                        } else {
                            ui.colored_label(case_color(i as u64), text);
                        }
                    }
                });
                ui.end_row();
            }
        });
    if p.outputs
        .iter()
        .any(|(_, values)| values.iter().map(|(_, c)| c).sum::<usize>() > p.samples)
    {
        ui.label(
            RichText::new("The decision collects a list: a case can get several values.")
                .small()
                .weak(),
        );
    }
    if p.no_match > 0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "No rule matches in {:.1} % of the cases; they get nothing (null).",
                share(p.no_match)
            ),
        );
    }
    if p.violations > 0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Several rules match in {:.1} % of the cases; the first one is used.",
                share(p.violations)
            ),
        );
    }
    for (input, why) in &p.unknown {
        ui.label(
            RichText::new(format!("{input}: {why}, taken as empty here."))
                .small()
                .weak(),
        );
    }
}
