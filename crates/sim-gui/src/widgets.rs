//! Small editing widgets.

use egui::{ComboBox, DragValue, Id, Ui};
use sim_core::dist::{DistSpec, Seconds};

const UNITS: [(&str, f64); 5] = [
    ("s", 1.0),
    ("m", 60.0),
    ("h", 3600.0),
    ("d", 86_400.0),
    ("w", 604_800.0),
];

fn natural_unit(secs: f64) -> usize {
    UNITS
        .iter()
        .rposition(|(_, f)| secs > 0.0 && (secs / f).fract() == 0.0)
        .unwrap_or(0)
}

/// Amount plus unit picker. The chosen unit is remembered per widget id.
pub fn duration_edit(ui: &mut Ui, id: Id, value: &mut Seconds) {
    let unit_id = id.with("unit");
    let mut unit = ui
        .data_mut(|d| d.get_temp::<usize>(unit_id))
        .unwrap_or_else(|| natural_unit(value.0));
    let factor = UNITS[unit].1;
    let mut amount = value.0 / factor;
    if ui
        .add(DragValue::new(&mut amount).speed(0.1).range(0.0..=f64::MAX))
        .changed()
    {
        value.0 = amount * factor;
    }
    let before = unit;
    ComboBox::from_id_salt(id.with("combo"))
        .width(40.0)
        .selected_text(UNITS[unit].0)
        .show_ui(ui, |ui| {
            for (i, (name, _)) in UNITS.iter().enumerate() {
                ui.selectable_value(&mut unit, i, *name);
            }
        });
    if unit != before {
        ui.data_mut(|d| d.insert_temp(unit_id, unit));
    }
}

const KINDS: [&str; 6] = [
    "fixed",
    "uniform",
    "normal",
    "exponential",
    "triangular",
    "lognormal",
];

fn kind_index(d: &DistSpec) -> usize {
    match d {
        DistSpec::Fixed { .. } => 0,
        DistSpec::Uniform { .. } => 1,
        DistSpec::Normal { .. } => 2,
        DistSpec::Exponential { .. } => 3,
        DistSpec::Triangular { .. } => 4,
        DistSpec::Lognormal { .. } => 5,
    }
}

/// A representative value of the distribution, used to pick sensible defaults when switching kind.
fn centre(d: &DistSpec) -> f64 {
    match d {
        DistSpec::Fixed { value } => value.0,
        DistSpec::Uniform { min, max } => (min.0 + max.0) / 2.0,
        DistSpec::Normal { mean, .. }
        | DistSpec::Exponential { mean }
        | DistSpec::Lognormal { mean, .. } => mean.0,
        DistSpec::Triangular { mode, .. } => mode.0,
    }
}

pub fn default_dist(kind: usize, c: f64) -> DistSpec {
    let s = Seconds;
    match kind {
        0 => DistSpec::Fixed { value: s(c) },
        1 => DistSpec::Uniform {
            min: s(c * 0.5),
            max: s(c * 1.5),
        },
        2 => DistSpec::Normal {
            mean: s(c),
            stdev: s(c * 0.2),
        },
        3 => DistSpec::Exponential { mean: s(c) },
        4 => DistSpec::Triangular {
            min: s(c * 0.5),
            mode: s(c),
            max: s(c * 2.0),
        },
        _ => DistSpec::Lognormal {
            mean: s(c),
            stdev: s(c * 0.5),
        },
    }
}

/// Distribution picker with the fields of the chosen kind, in one row.
pub fn dist_edit(ui: &mut Ui, id: Id, dist: &mut DistSpec) {
    ui.horizontal(|ui| {
        let mut kind = kind_index(dist);
        let before = kind;
        ComboBox::from_id_salt(id.with("kind"))
            .width(90.0)
            .selected_text(KINDS[kind])
            .show_ui(ui, |ui| {
                for (i, k) in KINDS.iter().enumerate() {
                    ui.selectable_value(&mut kind, i, *k);
                }
            });
        if kind != before {
            *dist = default_dist(kind, centre(dist));
        }
        let field = |ui: &mut Ui, label: &str, v: &mut Seconds| {
            ui.label(label);
            duration_edit(ui, id.with(label), v);
        };
        match dist {
            DistSpec::Fixed { value } => field(ui, "value", value),
            DistSpec::Uniform { min, max } => {
                field(ui, "min", min);
                field(ui, "max", max);
            }
            DistSpec::Normal { mean, stdev } | DistSpec::Lognormal { mean, stdev } => {
                field(ui, "mean", mean);
                field(ui, "stdev", stdev);
            }
            DistSpec::Exponential { mean } => field(ui, "mean", mean),
            DistSpec::Triangular { min, mode, max } => {
                field(ui, "min", min);
                field(ui, "mode", mode);
                field(ui, "max", max);
            }
        }
    });
}
