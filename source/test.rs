use core::f64;
use std::fmt::Debug;

use crate::{line_point_distance, SerializablePoint, Transform};


#[test]
fn test_line_point_distance() {
    let start = SerializablePoint {x: 1., y: 1.};
    let end = SerializablePoint {x: 2., y: 2.};
    assert_approx_eq(&line_point_distance(&start, &end, &SerializablePoint {x: 1., y: 1.}), &0., 1e-14);
    assert_approx_eq(&line_point_distance(&start, &end, &SerializablePoint {x: 1., y: 2.}), &f64::sqrt(0.5), 1e-14);
    assert_approx_eq(&line_point_distance(&start, &end, &SerializablePoint {x: 0., y: 0.}), &f64::sqrt(2.0), 1e-14);
    assert_approx_eq(&line_point_distance(&start, &end, &SerializablePoint {x: 2., y: 3.}), &1., 1e-14);
}


#[test]
fn test_transverse_transform() {
    let transform = Transform::Oblique {
        pole_longitude: 0., pole_latitude: 0.,
        projection: std::boxed::Box::new(Transform::Affine {
            x_scale: 1., x_shift: 0.,
            y_scale: 1., y_shift: 0.,
        }),
    };
    assert_approx_eq(
        &transform.apply(&SerializablePoint { x: 90., y: 0. }),
        &SerializablePoint { x: 90., y: 0. },
        1e-14,
    );
    assert_approx_eq(
        &transform.apply(&SerializablePoint { x: 0., y: -90. }),
        &SerializablePoint { x: 0., y: 0. },
        1e-14,
    );
    assert_approx_eq(
        &transform.apply(&SerializablePoint { x: 1., y: 1. }),
        &SerializablePoint { x: 135., y: 90. - f64::sqrt(2.) },
        1e-2,
    );
}


#[test]
fn test_oblique_transform() {
    let transform = Transform::Oblique {
        pole_longitude: 90., pole_latitude: 60.,
        projection: std::boxed::Box::new(Transform::Affine {
            x_scale: 1., x_shift: 0.,
            y_scale: 1., y_shift: 0.,
        }),
    };
    assert_approx_eq(
        &transform.apply(&SerializablePoint { x: 90., y: 0. }),
        &SerializablePoint { x: 0., y: 30. },
        1e-14,
    );
    assert_approx_eq(
        &transform.apply(&SerializablePoint { x: 0., y: 0. }),
        &SerializablePoint { x: -90., y: 0. },
        1e-14,
    );
    assert_approx_eq(
        &transform.apply(&SerializablePoint { x: 90.02, y: 60.01 }),
        &SerializablePoint { x: 135., y: 90. - f64::sqrt(0.0002) },
        1e-1,
    );
}


fn assert_approx_eq<T: ApproxEq + Debug>(a: &T, b: &T, abs_tolerance: f64) {
    if !a.approx_eq(b, abs_tolerance) {
        panic!(
            "these two values were not equal to within the given absolute tolerance ({}):\n\t{:?}\n\t{:?}",
            abs_tolerance, a, b);
    }
}


pub trait ApproxEq {
    fn approx_eq(self: &Self, other: &Self, abs_tolerance: f64) -> bool;
}

impl ApproxEq for f64 {
    fn approx_eq(self: &Self, other: &Self, abs_tolerance: f64) -> bool {
        return f64::abs(self - other) <= abs_tolerance;
    }
}

impl ApproxEq for SerializablePoint {
    fn approx_eq(self: &Self, other: &Self, abs_tolerance: f64) -> bool {
        return self.x.approx_eq(&other.x, abs_tolerance) && self.y.approx_eq(&other.y, abs_tolerance);
    }
}