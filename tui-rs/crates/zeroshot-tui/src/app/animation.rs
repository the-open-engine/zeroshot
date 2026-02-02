use std::f32::consts::TAU;

pub const DEFAULT_TICK_MS: i64 = 250;
pub const MIN_TICK_MS: i64 = 16;
pub const MAX_TICK_MS: i64 = 1000;
pub const PHASE_TICKS: u64 = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimClock {
    pub now_ms: i64,
    pub tick: u64,
    pub phase: f32,
}

impl Default for AnimClock {
    fn default() -> Self {
        Self {
            now_ms: 0,
            tick: 0,
            phase: 0.0,
        }
    }
}

impl AnimClock {
    pub fn advance(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
        self.tick = self.tick.saturating_add(1);
        self.phase = ((self.tick % PHASE_TICKS) as f32) / (PHASE_TICKS as f32);
    }
}

pub fn pulse_factor(phase: f32) -> f32 {
    0.5 + 0.5 * (phase * TAU).sin()
}

pub fn smooth_factor(dt_ms: i64, base: f64) -> f64 {
    let dt_scale = (dt_ms as f64 / DEFAULT_TICK_MS as f64).clamp(0.2, 2.0);
    (base * dt_scale).clamp(0.0, 1.0)
}

pub fn lerp_f64(current: f64, target: f64, t: f64) -> f64 {
    current + (target - current) * t
}

pub fn smooth_toward_f64(current: f64, target: f64, dt_ms: i64, rate: f64) -> f64 {
    let t = smooth_factor(dt_ms, rate);
    lerp_f64(current, target, t)
}

pub fn step_spring_f32(
    position: (f32, f32),
    velocity: (f32, f32),
    target: (f32, f32),
    dt_ms: i64,
    accel: f32,
    friction: f32,
) -> ((f32, f32), (f32, f32)) {
    let dt_scale = (dt_ms as f32 / DEFAULT_TICK_MS as f32).clamp(0.2, 2.0);
    let accel_step = accel * dt_scale;
    let friction_step = friction.powf(dt_scale);

    let mut vx = velocity.0 + (target.0 - position.0) * accel_step;
    let mut vy = velocity.1 + (target.1 - position.1) * accel_step;
    vx *= friction_step;
    vy *= friction_step;

    let px = position.0 + vx * dt_scale;
    let py = position.1 + vy * dt_scale;

    ((px, py), (vx, vy))
}

pub fn step_spring_f64(
    position: (f64, f64),
    velocity: (f64, f64),
    target: (f64, f64),
    dt_ms: i64,
    accel: f64,
    friction: f64,
) -> ((f64, f64), (f64, f64)) {
    let dt_scale = (dt_ms as f64 / DEFAULT_TICK_MS as f64).clamp(0.2, 2.0);
    let accel_step = accel * dt_scale;
    let friction_step = friction.powf(dt_scale);

    let mut vx = velocity.0 + (target.0 - position.0) * accel_step;
    let mut vy = velocity.1 + (target.1 - position.1) * accel_step;
    vx *= friction_step;
    vy *= friction_step;

    let px = position.0 + vx * dt_scale;
    let py = position.1 + vy * dt_scale;

    ((px, py), (vx, vy))
}

pub fn clamp_tick_delta(last_tick_ms: Option<i64>, now_ms: i64) -> i64 {
    let raw = last_tick_ms
        .map(|last| now_ms.saturating_sub(last))
        .unwrap_or(DEFAULT_TICK_MS);
    raw.clamp(MIN_TICK_MS, MAX_TICK_MS)
}
