use zeroshot_tui::app::animation::{pulse_factor, step_spring_f32, AnimClock, PHASE_TICKS};

#[test]
fn anim_clock_advances_and_wraps() {
    let mut clock = AnimClock::default();
    clock.advance(100);
    let first_phase = clock.phase;
    assert_eq!(clock.tick, 1);
    assert_eq!(clock.now_ms, 100);

    for _ in 0..PHASE_TICKS {
        clock.advance(200);
    }

    assert_eq!(clock.tick, 1 + PHASE_TICKS);
    assert!((clock.phase - first_phase).abs() < 1e-6);
}

#[test]
fn camera_smoothing_moves_toward_target() {
    let position = (0.0_f32, 0.0_f32);
    let velocity = (0.0_f32, 0.0_f32);
    let target = (10.0_f32, 0.0_f32);

    let (pos1, vel1) = step_spring_f32(position, velocity, target, 250, 0.16, 0.82);
    assert!(pos1.0 > 0.0);
    assert!(vel1.0 > 0.0);

    let (pos2, _vel2) = step_spring_f32(pos1, vel1, target, 250, 0.16, 0.82);
    assert!(pos2.0 > pos1.0);
}

#[test]
fn error_pulse_varies_with_phase() {
    let start = pulse_factor(0.0);
    let mid = pulse_factor(0.25);
    assert!((start - mid).abs() > 0.1);
    assert!((0.0..=1.0).contains(&start));
    assert!((0.0..=1.0).contains(&mid));
}
