use anyhow::{anyhow, Result};
use std::{env, f64::consts::PI};
use std::fmt::Debug;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        return Err(anyhow!(
            "you must pass exactly three arguments: the latitude and longitude of the point you
            want in the center of the map and the angle you want the compass to be rotated clockwise."));
    }
    let latitude: f64 = str::parse(&args[1])?;
    let longitude: f64 = str::parse(&args[2])?;
    let rotation: f64 = str::parse(&args[3])?;
    let (pole_latitude, pole_longitude, central_meridian) = inverse_oblique_transform(
        latitude, longitude, 0., 0., rotation,
    );
    println!("to make the point");
    println!("\t{:.2}°N {:.2}°E", latitude, longitude);
    println!("appear at the center of the map with the north vector rotated");
    println!("\t{:.2}° clockwise,", rotation);
    println!("you must use an oblique map projection with its pole at");
    println!("\t{:.2}°N {:.2}°E", pole_latitude, pole_longitude);
    println!("and central meridian");
    println!("\t{:.2}°E", central_meridian);
    return Ok(());
}

/// given a starting point and a starting bearing, calculate the coordinate system you need to
/// put it in a given place with a given rotation.
/// - φ_point: the latitude of the point in the global coordinate system, in degrees
/// - λ_point: the longitude of the point in the global coordinate system, in degrees
/// - φ_relative: the desired latitude of the point in the new coordinate system, in degrees
/// - λ_relative: the desired longitude of the point in the new coordinate system, in degrees
/// - rotation: the desired angular offset between the coordinate systems at the given point,
///             in degrees (a positive number means true north is rotated clockwise relative
///             to "north" towards the new pole)
/// returns the pole latitude, pole longitude, and central meridian to achieve the desired effect
fn inverse_oblique_transform(
    φ_point: f64, λ_point: f64,
    φ_relative: f64, λ_relative: f64, rotation: f64
) -> (f64, f64, f64) {
    // convert everything to radians
    let φ_point = φ_point.to_radians();
    let λ_point = λ_point.to_radians();
    let φ_relative = φ_relative.to_radians();
    let λ_relative = λ_relative.to_radians();
    let rotation = rotation.to_radians();
    // change t osome alternative coordinate representations
    let distance = PI/2. - φ_relative;
    let initial_bearing = -rotation;
    // this is the unit vector that points from the center of the sphere to the given point
    let up_y = -φ_point.cos();
    let up_z = φ_point.sin();
    // this is the unit vector that points east at the given point
    let east_x = 1.;
    // this is the unit vector that points north at the given point
    let north_y = up_z;
    let north_z = -up_y;
    // this is the unit vector that points from the given point, tangent along the path to the new pole
    let forth_x = east_x*initial_bearing.sin();
    let forth_y = north_y*initial_bearing.cos();
    let forth_z = north_z*initial_bearing.cos();
    // this is a vector that points from the center of the sphere to the new pole
    let pole_x = forth_x*distance.sin();
    let pole_y = forth_y*distance.sin() + up_y*distance.cos();
    let pole_z = forth_z*distance.sin() + up_z*distance.cos();
    // now we can calculate the pole coordinates
    let φ_pole = f64::atan2(pole_z, f64::hypot(pole_x, pole_y));
    let λ_pole = f64::atan2(pole_x, -pole_y);
    // this is the unit vector that points east at the new pole (true east, mind you)
    let pole_east_y = λ_pole.sin(); // (no need calculate x because we're dotting it with up
    // this is the unit vector that points north at the new pole (true north, mind you)
    let pole_north_y = φ_pole.sin()*λ_pole.cos(); // (no need calculate x because we're dotting it with up)
    let pole_north_z = φ_pole.cos();
    // now we can calculate the bearing from the new pole toward the given point
    let central_meridian = f64::atan2(pole_east_y*up_y, -(pole_north_y*up_y + pole_north_z*up_z)) - λ_relative;
    // this whole time we've implicitly used a rotated reference frame where the point has λ=0.  correct that.
    let λ_pole = λ_point + λ_pole;
    // convert back to degrees and return
    return (φ_pole.to_degrees(), λ_pole.to_degrees(), central_meridian.to_degrees());
}

#[test]
fn test_inverse_oblique_transform() {
    assert_approx_eq(&inverse_oblique_transform(0., 90., 30., 0., 0.), &(60., 90., 0.), 1e-14);
    assert_approx_eq(&inverse_oblique_transform(0., 0., 0., -90., -30.), &(60., 90., 0.), 1e-14);
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

impl ApproxEq for (f64, f64, f64) {
    fn approx_eq(self: &Self, other: &Self, abs_tolerance: f64) -> bool {
        return self.0.approx_eq(&other.0, abs_tolerance) &&
               self.1.approx_eq(&other.1, abs_tolerance) &&
               self.2.approx_eq(&other.2, abs_tolerance);
    }
}

impl ApproxEq for f64 {
    fn approx_eq(self: &Self, other: &Self, abs_tolerance: f64) -> bool {
        return f64::abs(self - other) <= abs_tolerance;
    }
}
