//! Durations ("30m") and probability distributions. All time is `f64` seconds.

use rand::Rng;
use rand_distr::{Distribution, Exp, LogNormal, Normal, Triangular, Uniform};
use serde::{Deserialize, Deserializer};

/// A duration in seconds. Deserialises from `"30m"` (units s, m, h, d, w) or a plain number of seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seconds(pub f64);

pub fn parse_duration(text: &str) -> Result<f64, String> {
    let t = text.trim();
    let split = t
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| format!("duration '{text}' has no unit (use s, m, h, d or w)"))?;
    let (num, unit) = t.split_at(split);
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("duration '{text}': '{}' is not a number", num.trim()))?;
    let factor = match unit {
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86_400.0,
        "w" => 604_800.0,
        _ => {
            return Err(format!(
                "duration '{text}': unknown unit '{unit}' (use s, m, h, d or w)"
            ));
        }
    };
    if !value.is_finite() || value < 0.0 {
        return Err(format!("duration '{text}' must be a non-negative number"));
    }
    Ok(value * factor)
}

impl<'de> Deserialize<'de> for Seconds {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Int(i64),
            Float(f64),
        }
        match Raw::deserialize(d)? {
            Raw::Text(s) => parse_duration(&s)
                .map(Seconds)
                .map_err(serde::de::Error::custom),
            Raw::Int(i) => Ok(Seconds(i as f64)),
            Raw::Float(f) => Ok(Seconds(f)),
        }
    }
}

/// Distribution as written in a scenario file.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "dist", rename_all = "lowercase", deny_unknown_fields)]
pub enum DistSpec {
    Fixed {
        value: Seconds,
    },
    Uniform {
        min: Seconds,
        max: Seconds,
    },
    Normal {
        mean: Seconds,
        stdev: Seconds,
    },
    Exponential {
        mean: Seconds,
    },
    Triangular {
        min: Seconds,
        mode: Seconds,
        max: Seconds,
    },
    Lognormal {
        mean: Seconds,
        stdev: Seconds,
    },
}

/// A validated, ready-to-sample distribution.
#[derive(Debug, Clone)]
pub enum Dist {
    Fixed(f64),
    Uniform(Uniform<f64>),
    Normal(Normal<f64>),
    Exponential(Exp<f64>),
    Triangular(Triangular<f64>),
    LogNormal(LogNormal<f64>),
}

impl Dist {
    pub fn new(spec: &DistSpec) -> Result<Self, String> {
        let bad = |what: &str, e: &dyn std::fmt::Display| format!("{what}: {e}");
        Ok(match spec {
            DistSpec::Fixed { value } => Dist::Fixed(value.0),
            DistSpec::Uniform { min, max } => {
                Dist::Uniform(Uniform::new_inclusive(min.0, max.0).map_err(|e| bad("uniform", &e))?)
            }
            DistSpec::Normal { mean, stdev } => {
                Dist::Normal(Normal::new(mean.0, stdev.0).map_err(|e| bad("normal", &e))?)
            }
            DistSpec::Exponential { mean } => {
                if mean.0 <= 0.0 {
                    return Err("exponential: mean must be positive".into());
                }
                Dist::Exponential(Exp::new(1.0 / mean.0).map_err(|e| bad("exponential", &e))?)
            }
            DistSpec::Triangular { min, mode, max } => Dist::Triangular(
                Triangular::new(min.0, max.0, mode.0).map_err(|e| bad("triangular", &e))?,
            ),
            DistSpec::Lognormal { mean, stdev } => {
                if mean.0 <= 0.0 {
                    return Err("lognormal: mean must be positive".into());
                }
                // mean/stdev of the lognormal itself -> parameters of the underlying normal
                let sigma2 = (1.0 + (stdev.0 / mean.0).powi(2)).ln();
                let mu = mean.0.ln() - sigma2 / 2.0;
                Dist::LogNormal(
                    LogNormal::new(mu, sigma2.sqrt()).map_err(|e| bad("lognormal", &e))?,
                )
            }
        })
    }

    /// Draw one sample, cut off at 0.
    pub fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> f64 {
        let x = match self {
            Dist::Fixed(v) => *v,
            Dist::Uniform(d) => d.sample(rng),
            Dist::Normal(d) => d.sample(rng),
            Dist::Exponential(d) => d.sample(rng),
            Dist::Triangular(d) => d.sample(rng),
            Dist::LogNormal(d) => d.sample(rng),
        };
        x.max(0.0)
    }
}
