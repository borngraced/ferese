use crate::{CrossingPolicy, SpringConfig};

/// One configuration contract for compositor and out-of-process shell motion.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MotionSettings {
    pub enabled: bool,
    pub reduced_motion: bool,
    pub speed: f64,
    pub spring: SpringSettings,
    pub viewport_spring: SpringSettings,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced_motion: false,
            speed: 1.0,
            spring: SpringSettings::default(),
            viewport_spring: SpringSettings::default(),
        }
    }
}

impl MotionSettings {
    pub fn motion_enabled(self) -> bool {
        self.enabled && !self.reduced_motion
    }

    pub fn validate(self) -> Result<(), String> {
        if !self.speed.is_finite() || self.speed <= 0.0 {
            return Err("animation speed must be finite and positive".into());
        }
        self.spring.resolve(SpringConfig::default())?;
        self.viewport_config()?;
        Ok(())
    }

    pub fn viewport_config(self) -> Result<SpringConfig, String> {
        let mut settings = self.viewport_spring;
        if settings.duration_ms.is_none() && settings.damping.is_none() && settings.damping_ratio.is_none() {
            settings.damping_ratio = Some(1.0);
        }
        settings.resolve(SpringConfig {
            stiffness: 320.0,
            damping: 2.0 * 320.0_f64.sqrt(),
            ..SpringConfig::default()
        })
    }

    /// Only small opacity changes should use a timed duration.
    pub fn duration(self, milliseconds: f64) -> std::time::Duration {
        if !self.motion_enabled() {
            return std::time::Duration::ZERO;
        }
        std::time::Duration::from_secs_f64(milliseconds / 1000.0 / self.speed)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpringSettings {
    pub mass: Option<f64>,
    pub stiffness: Option<f64>,
    pub damping: Option<f64>,
    pub damping_ratio: Option<f64>,
    pub duration_ms: Option<f64>,
    pub bounce: Option<f64>,
    pub overshoot: bool,
}

impl SpringSettings {
    pub fn resolve(self, base: SpringConfig) -> Result<SpringConfig, String> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        let mut config = SpringConfig {
            mass: self.mass.unwrap_or(base.mass),
            stiffness: self.stiffness.unwrap_or(base.stiffness),
            damping: self.damping.unwrap_or(base.damping),
            crossing: if self.overshoot {
                CrossingPolicy::AllowOvershoot
            } else {
                CrossingPolicy::NoCrossing
            },
            ..base
        };
        if !positive(config.mass) || !positive(config.stiffness) || !config.damping.is_finite() || config.damping < 0.0
        {
            return Err("spring mass/stiffness must be positive and damping nonnegative".into());
        }
        if let Some(duration) = self.duration_ms {
            if self.stiffness.is_some() || self.damping.is_some() || self.damping_ratio.is_some() {
                return Err("spring duration-ms cannot be combined with stiffness/damping/damping-ratio".into());
            }
            let bounce = self.bounce.unwrap_or(0.0);
            if !positive(duration)
                || !bounce.is_finite()
                || (bounce <= -1.0 || bounce >= 1.0)
                || (bounce > 0.0 && !self.overshoot)
            {
                return Err("spring duration-ms must be positive; bounce must be between -1 and 1 (exclusive), and positive bounce requires overshoot".into());
            }
            let ratio = if bounce >= 0.0 {
                1.0 - bounce
            } else {
                1.0 / (1.0 + bounce)
            };
            let omega = std::f64::consts::TAU / (duration / 1000.0);
            config.stiffness = config.mass * omega * omega;
            config.damping = 2.0 * config.mass * omega * ratio;
        } else {
            if self.bounce.is_some() {
                return Err("spring bounce requires duration-ms".into());
            }
            if let Some(ratio) = self.damping_ratio {
                if self.damping.is_some() || !ratio.is_finite() || ratio < 0.0 {
                    return Err("spring damping-ratio must be nonnegative and cannot be combined with damping".into());
                }
                config.damping = 2.0 * ratio * (config.mass * config.stiffness).sqrt();
            }
        }
        if !positive(config.stiffness) || !config.damping.is_finite() {
            return Err("spring coefficients overflow".into());
        }
        // Presentation lifetimes can depend on settlement. An undamped spring
        // only settles when the first-crossing clamp is enabled.
        if config.damping == 0.0 && config.crossing == CrossingPolicy::AllowOvershoot {
            return Err("spring overshoot requires positive damping so motion can settle".into());
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undamped_overshoot_is_rejected_for_both_motion_properties() {
        for spring in [
            SpringSettings {
                damping: Some(0.0),
                overshoot: true,
                ..Default::default()
            },
            SpringSettings {
                damping_ratio: Some(0.0),
                overshoot: true,
                ..Default::default()
            },
        ] {
            assert!(
                MotionSettings {
                    spring,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
            assert!(
                MotionSettings {
                    viewport_spring: spring,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            SpringSettings {
                overshoot: true,
                ..Default::default()
            }
            .resolve(SpringConfig {
                damping: 0.0,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn undamped_motion_needs_the_crossing_clamp_to_settle() {
        use crate::AnimatedValue;
        let clamped = SpringSettings {
            damping: Some(0.0),
            ..Default::default()
        }
        .resolve(SpringConfig::default())
        .unwrap();
        let mut closing = AnimatedValue {
            current: 1.0,
            target: 0.0,
            velocity: 0.0,
        };
        let mut free = closing;
        for _ in 0..3600 {
            let dt = std::time::Duration::from_secs_f64(1.0 / 60.0);
            closing.advance(dt, clamped);
            free.advance(
                dt,
                SpringConfig {
                    crossing: CrossingPolicy::AllowOvershoot,
                    ..clamped
                },
            );
            assert!(free.current != free.target || free.velocity != 0.0);
        }
        assert_eq!(closing.current, closing.target);
        assert_eq!(closing.velocity, 0.0);
    }

    #[test]
    fn viewport_legacy_damping_tracks_overridden_coefficients() {
        let settings = MotionSettings {
            viewport_spring: SpringSettings {
                mass: Some(2.0),
                stiffness: Some(500.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let spring = settings.viewport_config().unwrap();
        assert_eq!(spring.damping, 2.0 * 1000.0_f64.sqrt());
        assert_eq!(spring.crossing, CrossingPolicy::NoCrossing);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn policy_applies_speed_and_reduced_motion_consistently() {
        let slow = MotionSettings {
            speed: 0.5,
            ..Default::default()
        };
        assert_eq!(slow.duration(120.0), std::time::Duration::from_millis(240));
        assert!(
            !MotionSettings {
                reduced_motion: true,
                ..slow
            }
            .motion_enabled()
        );
        assert!(
            MotionSettings {
                reduced_motion: true,
                ..slow
            }
            .duration(120.0)
            .is_zero()
        );
        assert!(MotionSettings { speed: 0.0, ..slow }.validate().is_err());
    }
    #[test]
    fn perceptual_conversion_and_legacy_defaults() {
        let base = SpringConfig::default();
        assert_eq!(SpringSettings::default().resolve(base).unwrap(), base);
        for (bounce, ratio) in [(0.0, 1.0), (0.3, 0.7), (-0.5, 2.0)] {
            let spring = SpringSettings {
                duration_ms: Some(500.0),
                bounce: Some(bounce),
                overshoot: bounce > 0.0,
                ..Default::default()
            }
            .resolve(base)
            .unwrap();
            assert!((spring.stiffness - 157.913670417).abs() < 1e-8);
            assert!((spring.damping / (2.0 * spring.stiffness.sqrt()) - ratio).abs() < 1e-10);
            assert!((std::f64::consts::TAU * (spring.mass / spring.stiffness).sqrt() - 0.5).abs() < 1e-10);
        }
    }
    #[test]
    fn rejects_ambiguous_or_nonsettling_configuration() {
        for settings in [
            SpringSettings {
                duration_ms: Some(300.0),
                stiffness: Some(100.0),
                ..Default::default()
            },
            SpringSettings {
                duration_ms: Some(300.0),
                bounce: Some(0.3),
                ..Default::default()
            },
            SpringSettings {
                duration_ms: Some(300.0),
                bounce: Some(-1.0),
                ..Default::default()
            },
            SpringSettings {
                duration_ms: Some(300.0),
                bounce: Some(1.0),
                overshoot: true,
                ..Default::default()
            },
        ] {
            assert!(settings.resolve(SpringConfig::default()).is_err());
        }
    }
}
