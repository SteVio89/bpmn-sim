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

/// A duration for reading, rounded to two significant digits in a fitting unit ("5m", "1.5h").
pub fn short_duration(secs: f64) -> String {
    let (v, unit) = if secs >= 86_400.0 {
        (secs / 86_400.0, "d")
    } else if secs >= 3600.0 {
        (secs / 3600.0, "h")
    } else if secs >= 60.0 {
        (secs / 60.0, "m")
    } else {
        (secs, "s")
    };
    let v = if v >= 10.0 {
        v.round()
    } else {
        (v * 10.0).round() / 10.0
    };
    format!("{v}{unit}")
}

/// "5m" for a fixed value, "~5m" (the mean) for a random one.
pub fn dist_short(d: &DistSpec) -> String {
    match d {
        DistSpec::Fixed { value } => short_duration(value.0),
        _ => format!("~{}", short_duration(d.mean())),
    }
}

/// How often something happens with this time between events: "about 5 per hour, 40 per day".
pub fn rate_text(d: &DistSpec) -> String {
    let mean = d.mean();
    if mean <= 0.0 {
        return "all at once".into();
    }
    let fmt = |x: f64| {
        if x >= 10.0 {
            format!("{}", x.round())
        } else {
            format!("{}", (x * 10.0).round() / 10.0)
        }
    };
    let (per_hour, per_day) = (3600.0 / mean, 86_400.0 / mean);
    if per_hour >= 1.0 {
        format!("about {} per hour, {} per day", fmt(per_hour), fmt(per_day))
    } else if per_day >= 1.0 {
        format!(
            "about {} per day, {} per week",
            fmt(per_day),
            fmt(per_day * 7.0)
        )
    } else {
        format!("about {} per week", fmt(per_day * 7.0))
    }
}

/// Sorted preview samples of a distribution, cached per distribution.
fn samples(ui: &Ui, d: &DistSpec) -> Option<std::sync::Arc<Vec<f64>>> {
    let key = Id::unique(("dist-preview", format!("{d:?}")));
    if let Some(s) = ui.data(|m| m.get_temp::<std::sync::Arc<Vec<f64>>>(key)) {
        return Some(s);
    }
    let s = std::sync::Arc::new(d.preview(2000).ok()?);
    ui.data_mut(|m| m.insert_temp(key, s.clone()));
    Some(s)
}

/// A small histogram of the distribution with its mean, and the range most values fall in.
pub fn dist_preview(ui: &mut Ui, d: &DistSpec) {
    let Some(s) = samples(ui, d) else {
        ui.colored_label(ui.visuals().error_fg_color, "invalid distribution");
        return;
    };
    let at = |q: f64| s[((s.len() - 1) as f64 * q) as usize];
    let (lo, hi) = (at(0.01), at(0.99));
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0, 32.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let stroke = ui.visuals().widgets.noninteractive.fg_stroke.color;
        let fill = ui.visuals().selection.bg_fill;
        painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        if hi - lo < 1e-9 {
            // A fixed value: one bar.
            let x = rect.center().x;
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.top() + 2.0),
                ],
                egui::Stroke::new(3.0, fill),
            );
        } else {
            const BINS: usize = 28;
            let mut bins = [0u32; BINS];
            for v in s.iter().filter(|v| (lo..=hi).contains(*v)) {
                let i = (((v - lo) / (hi - lo)) * BINS as f64) as usize;
                bins[i.min(BINS - 1)] += 1;
            }
            let top = f64::from(*bins.iter().max().unwrap_or(&1).max(&1));
            let w = rect.width() / BINS as f32;
            for (i, c) in bins.iter().enumerate() {
                let h = (f64::from(*c) / top) as f32 * (rect.height() - 3.0);
                let r = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + i as f32 * w, rect.bottom() - h),
                    egui::pos2(rect.left() + (i + 1) as f32 * w - 0.5, rect.bottom()),
                );
                painter.rect_filled(r, 0.0, fill);
            }
            let x = rect.left() + ((mean - lo) / (hi - lo)) as f32 * rect.width();
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0, stroke),
            );
        }
        let text = if at(0.9) - at(0.1) < 1e-9 {
            format!("always {}", short_duration(mean))
        } else {
            format!(
                "usually {} to {}, average {}",
                short_duration(at(0.1)),
                short_duration(at(0.9)),
                short_duration(mean)
            )
        };
        ui.label(egui::RichText::new(text).small());
    });
}
