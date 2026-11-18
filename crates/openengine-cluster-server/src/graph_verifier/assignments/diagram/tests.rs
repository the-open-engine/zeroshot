use super::{ALWAYS, Diagram, NEVER, Ref};

fn predicates() -> (Diagram, [Ref; 4]) {
    let mut diagram = Diagram::default();
    let x = diagram.variable(3).unwrap();
    let y = diagram.variable(2).unwrap();
    let z = diagram.variable(2).unwrap();
    let x_zero = diagram.test(x, [ALWAYS, NEVER, NEVER]).unwrap();
    let x_one = diagram.test(x, [NEVER, ALWAYS, NEVER]).unwrap();
    let y_one = diagram.test(y, [NEVER, ALWAYS]).unwrap();
    let z_one = diagram.test(z, [NEVER, ALWAYS]).unwrap();
    (diagram, [x_zero, x_one, y_one, z_one])
}

fn evaluate(diagram: &Diagram, mut condition: Ref, values: [usize; 3]) -> bool {
    while condition > ALWAYS {
        let node = &diagram.nodes[condition as usize];
        condition = node.branches[values[node.variable as usize]];
    }
    condition == ALWAYS
}

fn assert_truth_table(diagram: &Diagram, condition: Ref, expected: impl Fn([usize; 3]) -> bool) {
    for x in 0..3 {
        for y in 0..2 {
            for z in 0..2 {
                let values = [x, y, z];
                assert_eq!(
                    evaluate(diagram, condition, values),
                    expected(values),
                    "assignment {values:?}"
                );
            }
        }
    }
}

#[test]
fn constants_and_empty_folds_have_boolean_identities() {
    let (mut diagram, [x_zero, _, _, _]) = predicates();
    assert_eq!(diagram.not(ALWAYS), Some(NEVER));
    assert_eq!(diagram.not(NEVER), Some(ALWAYS));
    assert_eq!(diagram.and(ALWAYS, x_zero), Some(x_zero));
    assert_eq!(diagram.and(x_zero, NEVER), Some(NEVER));
    assert_eq!(diagram.all([]), Some(ALWAYS));
    assert_eq!(diagram.any([]), Some(NEVER));
    assert_eq!(diagram.at_least(0, &[]), Some(ALWAYS));
    assert_eq!(diagram.at_least(1, &[]), Some(NEVER));
    assert_eq!(diagram.at_least(u64::MAX, &[x_zero]), Some(NEVER));
}

#[test]
fn conjunctions_preserve_shared_multivalued_variables() {
    let (mut diagram, [x_zero, x_one, y_one, z_one]) = predicates();
    let left = diagram.or(x_zero, y_one).unwrap();
    let right = diagram.or(x_zero, z_one).unwrap();
    let conjunction = diagram.and(left, right).unwrap();
    assert_truth_table(&diagram, conjunction, |[x, y, z]| {
        x == 0 || (y == 1 && z == 1)
    });
    assert_eq!(diagram.and(right, left), Some(conjunction));
    assert_eq!(diagram.and(left, left), Some(left));
    assert_eq!(diagram.and(x_zero, x_one), Some(NEVER));
    let distinct_variables = diagram.and(x_one, y_one).unwrap();
    assert_truth_table(&diagram, distinct_variables, |[x, y, _]| x == 1 && y == 1);
}

#[test]
fn negation_complements_shared_subconditions_and_is_an_involution() {
    let (mut diagram, [x_zero, _, y_one, z_one]) = predicates();
    let both = diagram.and(x_zero, y_one).unwrap();
    let condition = diagram.or(both, z_one).unwrap();
    let complement = diagram.not(condition).unwrap();
    assert_truth_table(&diagram, condition, |[x, y, z]| {
        (x == 0 && y == 1) || z == 1
    });
    assert_truth_table(&diagram, complement, |[x, y, z]| {
        !((x == 0 && y == 1) || z == 1)
    });
    assert_eq!(diagram.not(complement), Some(condition));
    assert_eq!(diagram.and(condition, complement), Some(NEVER));
    assert_eq!(diagram.or(condition, complement), Some(ALWAYS));
}

#[test]
fn choice_uses_the_same_assignment_for_its_condition_and_branches() {
    let (mut diagram, [x_zero, x_one, y_one, z_one]) = predicates();
    let then = diagram.or(x_one, y_one).unwrap();
    let otherwise = diagram.and(x_one, z_one).unwrap();
    let chosen = diagram.choose(x_zero, then, otherwise).unwrap();
    assert_truth_table(&diagram, chosen, |[x, y, z]| {
        if x == 0 {
            x == 1 || y == 1
        } else {
            x == 1 && z == 1
        }
    });
    assert_eq!(diagram.choose(ALWAYS, then, otherwise), Some(then));
    assert_eq!(diagram.choose(NEVER, then, otherwise), Some(otherwise));
}

#[test]
fn thresholds_count_repeated_complementary_and_shared_operands() {
    let (mut diagram, [x_zero, _, y_one, _]) = predicates();
    let not_x_zero = diagram.not(x_zero).unwrap();
    let not_y_one = diagram.not(y_one).unwrap();
    let both = diagram.and(x_zero, y_one).unwrap();
    let conditions = [
        x_zero, x_zero, not_x_zero, y_one, not_y_one, both, ALWAYS, NEVER,
    ];
    let mut reversed = conditions;
    reversed.reverse();
    let mut rotated = conditions;
    rotated.rotate_left(3);
    for ordered in [conditions, reversed, rotated] {
        for threshold in 0..=ordered.len() + 1 {
            let condition = diagram.at_least(threshold as u64, &ordered).unwrap();
            assert_truth_table(&diagram, condition, |[x, y, _]| {
                [
                    x == 0,
                    x == 0,
                    x != 0,
                    y == 1,
                    y != 1,
                    x == 0 && y == 1,
                    true,
                    false,
                ]
                .into_iter()
                .filter(|holds| *holds)
                .count()
                    >= threshold
            });
        }
    }
}

#[test]
fn threshold_boundaries_preserve_independent_and_exclusive_conditions() {
    let (mut diagram, [x_zero, x_one, y_one, z_one]) = predicates();
    let independent = [x_zero, y_one, z_one];
    for threshold in 0..=4 {
        let condition = diagram.at_least(threshold as u64, &independent).unwrap();
        assert_truth_table(&diagram, condition, |[x, y, z]| {
            [x == 0, y == 1, z == 1]
                .into_iter()
                .filter(|holds| *holds)
                .count()
                >= threshold
        });
    }
    let either_label = diagram.at_least(1, &[x_zero, x_one]).unwrap();
    assert_truth_table(&diagram, either_label, |[x, _, _]| x < 2);
    assert_eq!(diagram.at_least(2, &[x_zero, x_one]), Some(NEVER));
}
