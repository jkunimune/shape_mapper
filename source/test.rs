use core::f64;
use std::fmt::Debug;

use crate::{line_point_distance, Jacobian, SerializablePoint, Transform};


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
            longitudinal_scale: 1e-3, false_easting: 0., // NOTE: Affine currently assumes units of m so this combo is a little sketchy right now
            latitudinal_scale: 1e-3, false_northing: 0.,
            rotation: 0.,
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
    assert_approx_eq(
        &transform.jacobian(&SerializablePoint { x: 90., y: 0. }),
        &Jacobian { dx_dx: 0., dy_dx: -1., dx_dy: 1., dy_dy: 0. },
        1e-14,
    );
}


#[test]
fn test_oblique_transform() {
    let transform = Transform::Oblique {
        pole_longitude: 90., pole_latitude: 60.,
        projection: std::boxed::Box::new(Transform::Affine {
            longitudinal_scale: 1e-3, false_easting: 0.,
            latitudinal_scale: 1e-3, false_northing: 0.,
            rotation: 0.,
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
    assert_approx_eq(
        &transform.jacobian(&SerializablePoint { x: 90., y: 0. }),
        &Jacobian { dx_dx: 2./f64::sqrt(3.), dy_dx: 0., dx_dy: 0., dy_dy: 1. },
        1e-14,
    );
}


#[test]
fn test_transform_jacobian() {
    let transform = Transform::Oblique {
        pole_longitude: 162.9, pole_latitude: 32.4,
        projection: std::boxed::Box::new(Transform::Affine {
            longitudinal_scale: 1e-3, false_easting: 0.,
            latitudinal_scale: 1e-3, false_northing: 0.,
            rotation: 0.,
        }),
    };
    let N = 10;
    let h = 1e-6;
    for i in 1..N {
        let φ = -90. + 180.*(i as f64)/(N as f64);
        for j in 0..N {
            let λ = -180. + 360.*(j as f64)/(N as f64);
            let center_point = transform.apply(&SerializablePoint { x: λ, y: φ });
            let east_point = transform.apply(&SerializablePoint { x: λ + h, y: φ });
            let north_point = transform.apply(&SerializablePoint { x: λ, y: φ + h });
            let approximate_jacobian = Jacobian {
                dx_dx: (east_point.x - center_point.x)/h,
                dy_dx: (east_point.y - center_point.y)/h,
                dx_dy: (north_point.x - center_point.x)/h,
                dy_dy: (north_point.y - center_point.y)/h,
            };
            let true_jacobian = transform.jacobian(&SerializablePoint { x: λ, y: φ });
            assert_approx_eq(
                &approximate_jacobian,
                &true_jacobian,
                1e-5,
            );
        }
    }
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

impl ApproxEq for Jacobian {
    fn approx_eq(self: &Self, other: &Self, abs_tolerance: f64) -> bool {
        return self.dx_dx.approx_eq(&other.dx_dx, abs_tolerance) &&
               self.dx_dy.approx_eq(&other.dx_dy, abs_tolerance) &&
               self.dy_dx.approx_eq(&other.dy_dx, abs_tolerance) &&
               self.dy_dy.approx_eq(&other.dy_dy, abs_tolerance);
    }
}
