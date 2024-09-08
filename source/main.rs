use core::f64;
use std::mem::discriminant;
use std::{fs, iter};
use serde::Deserialize;
use shapefile::dbase::FieldValue;
use shapefile::record::EsriShape;
use shapefile::Shape;


const IDENTITY_TRANSFORM: Transform = Transform { x_scale: 1., y_scale: 1., x_shift: 0., y_shift: 0. };


fn main() -> () {
    let yaml = fs::read_to_string(
        "configurations/congresentatives.yml").unwrap();
    let configuration: Configuration = serde_yaml::from_str(&yaml).unwrap();
    let configuration_width = f64::abs(configuration.bounding_box.right - configuration.bounding_box.left);
    let configuration_height = f64::abs(configuration.bounding_box.bottom - configuration.bounding_box.top);
    let mut svg_code = format!(
        "\
<svg viewBox=\"{} {} {} {}\" width=\"{}mm\" height=\"{}mm\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">
  <title>{}</title>
  <style>
{}
  </style>
\
        ",
        configuration.bounding_box.left, configuration.bounding_box.top,
        configuration_width, configuration_height,
        configuration_width, configuration_height,
        configuration.title, configuration.style,
    );
    svg_code.push_str(&transcribe_as_svg(&Content::from(configuration), 1, &IDENTITY_TRANSFORM).unwrap());
    svg_code.push_str("</svg>\n");

    let image_filename = "maps/congresentatives.svg";
    fs::create_dir_all("maps/").unwrap();
    fs::write(image_filename, svg_code).unwrap();
}


fn transcribe_as_svg(content: &Content, indent_level: usize, transform: &Transform) -> Result<String, MyError> {
    let indentation: String = iter::repeat("  ").take(indent_level).collect();
    let mut string = String::new();
    match content {
        Content::Group{ id, content: subcontents, bounding_box: output_bounding_box } => {
            let input_bounding_box = extent_of(subcontents)?;
            let transform = Transform::concatenate(
                &Transform::between(&input_bounding_box, &output_bounding_box), &transform);
            string.push_str(&format!(
                "{}<g id=\"{}\">\n",
                &indentation, &id));
            for subcontent in subcontents {
                string.push_str(&transcribe_as_svg(subcontent, indent_level + 1, &transform)?);
            }
        }
        Content::Layer{ filename, region, class, id_column } => {
            string.push_str(&format!(
                "{}<g id=\"{}\" class=\"{}\">\n",
                &indentation, &filename.to_lowercase(), &class));
            let mut reader = shapefile::Reader::from_path(
                format!("data/{}.shp", filename)).map_err(|err| MyError::new(err.to_string()))?;
            for shape_record in reader.iter_shapes_and_records() {
                let shape_record = shape_record.unwrap();
                let (shape, record) = &shape_record;
                // first, discard it if its bounding box doesn't intersect the desired region
                if !any_of_shape_is_in_box(shape, region) {
                    continue;
                }
                // come up with a useful class name
                let identifier: String = match id_column {
                    Some(id_column) => match record.get(id_column) {
                        Some(value) => match value {
                            FieldValue::Character(characters) => match characters {
                                Some(characters) => format!("id=\"{}\" ", characters.to_lowercase()),
                                None => String::from(""),
                            },
                            FieldValue::Numeric(number) => match number {
                                Some(number) => format!("id=\"{}-{}\" ", class, number),
                                None => String::from(""),
                            },
                            _ => return Err(MyError::new(String::from("I don't know how to print this field type."))),
                        },
                        None => return Err(MyError::new(format!("there doesn't seem to be a '{}' collum in '{}.shp'.", id_column, &filename))),
                    }
                    None => String::from(""),
                };
                match shape {
                    Shape::Polygon(polygon) => {
                        let mut path_string = String::new();
                        for ring in polygon.rings() {
                            for (i, point) in ring.points().iter().enumerate() {
                                let segment_type = if i == 0 {"M"} else {"L"};
                                let point = Transform::apply(transform, point);
                                path_string.push_str(&format!("{}{:.3},{:.3} ", segment_type, point.x, point.y));
                            }
                        }
                        string.push_str(&format!(
                            "{}  <path {}d=\"{}\" />\n",
                            indentation, identifier, path_string));
                    }
                    _ => {
                        panic!("we only do polygons right now.");
                    }
                }
            }
        }
    }
    string.push_str(&format!("{}</g>\n", &indentation));
    return Ok(string);
}


/**
 * determine the total bounding box of the given components.
 * the contents must all be in the same coordinate system or it'll return an Err.
 */
fn extent_of(contents: &Vec<Content>) -> Result<Box, MyError> {
    let mut x_min = f64::INFINITY;
    let mut x_max = -f64::INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = -f64::INFINITY;
    let mut group_system: Option<CoordinateSystem> = Option::None;
    for content in contents {
        // extract the bounding box from whatever it is
        let sub_extent = match content {
            Content::Layer { region, .. } => region,
            Content::Group { bounding_box, .. } => bounding_box,
        };
        // figure out whether we're using CS coordinates or math coordinates, if you haven't already
        let sub_system = if sub_extent.top > sub_extent.bottom {
            CoordinateSystem::YGoesUp
        } else {
            CoordinateSystem::YGoesDown
        };
        match &group_system {
            Some(system) => {
                if discriminant(system) != discriminant(&sub_system) {
                    return Result::Err(MyError::new(String::from("these contents had mismatched coordinate systems")));
                }
            }
            None => {
                group_system = Some(sub_system);
            }
        }
        // then expand the mutable variables as needed
        x_min = f64::min(x_min, sub_extent.left);
        x_max = f64::max(x_max, sub_extent.right);
        match &group_system {
            Some(CoordinateSystem::YGoesUp) => {
                y_min = f64::min(y_min, sub_extent.bottom);
                y_max = f64::max(y_max, sub_extent.top);
            }
            Some(CoordinateSystem::YGoesDown) => {
                y_min = f64::min(y_min, sub_extent.top);
                y_max = f64::max(y_max, sub_extent.bottom);
            }
            None => {
                panic!("I'm pretty sure this line is impossible.");
            }
        }
    }
    return Ok(match &group_system.unwrap() {
        CoordinateSystem::YGoesUp => Box { left: x_min, right: x_max, bottom: y_min, top: y_max },
        CoordinateSystem::YGoesDown => Box { left: x_min, right: x_max, top: y_min, bottom: y_max },
    });
}


fn any_of_shape_is_in_box(shape: &Shape, boks: &Box) -> bool {
    let (x_range, y_range) = match shape {
        Shape::NullShape => return false,
        Shape::Point(shape) => (shape.x_range(), shape.y_range()),
        Shape::PointM(shape) => (shape.x_range(), shape.y_range()),
        Shape::PointZ(shape) => (shape.x_range(), shape.y_range()),
        Shape::Polyline(shape) => (shape.x_range(), shape.y_range()),
        Shape::PolylineM(shape) => (shape.x_range(), shape.y_range()),
        Shape::PolylineZ(shape) => (shape.x_range(), shape.y_range()),
        Shape::Polygon(shape) => (shape.x_range(), shape.y_range()),
        Shape::PolygonM(shape) => (shape.x_range(), shape.y_range()),
        Shape::PolygonZ(shape) => (shape.x_range(), shape.y_range()),
        Shape::Multipoint(shape) => (shape.x_range(), shape.y_range()),
        Shape::MultipointM(shape) => (shape.x_range(), shape.y_range()),
        Shape::MultipointZ(shape) => (shape.x_range(), shape.y_range()),
        Shape::Multipatch(shape) => (shape.x_range(), shape.y_range()),
    };
    return
        x_range[0] < f64::max(boks.left, boks.right) &&
        x_range[1] > f64::min(boks.left, boks.right) &&
        y_range[0] < f64::max(boks.bottom, boks.top) &&
        y_range[1] > f64::min(boks.bottom, boks.top);
}


#[derive(Deserialize)]
struct Configuration {
    title: String,
    style: String,
    bounding_box: Box,
    content: Vec<Content>,
}


#[derive(Deserialize)]
enum Content {
    Layer {
        class: String,
        filename: String,
        region: Box,
        id_column: Option<String>,
    },
    Group {
        id: String,
        content: Vec<Content>,
        bounding_box: Box,
    },
}

impl From<Configuration> for Content {
    fn from(configuration: Configuration) -> Self {
        return Content::Group {
            id: String::from("content"),
            content: configuration.content,
            bounding_box: configuration.bounding_box
        };
    }
}


#[derive(Deserialize)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}


struct Transform {
    x_scale: f64,
    x_shift: f64,
    y_scale: f64,
    y_shift: f64,
}

impl Transform {
    fn between(input: &Box, output: &Box) -> Transform {
        let x_scale = (output.right - output.left)/(input.right - input.left);
        let x_shift = output.left - input.left*x_scale;
        let y_scale = (output.bottom - output.top)/(input.bottom - input.top);
        let y_shift = output.top - input.top*y_scale;
        return Transform { x_scale: x_scale, x_shift: x_shift, y_scale: y_scale, y_shift: y_shift };
    }

    fn concatenate(a: &Transform, b: &Transform) -> Transform {
        return Transform {
            x_scale: a.x_scale*b.x_scale,
            x_shift: a.x_shift*b.x_scale + b.x_shift,
            y_scale: a.y_scale*b.y_scale,
            y_shift: a.y_shift*b.y_scale + b.y_shift,
        }
    }

    fn apply(self: &Transform, point: &shapefile::Point) -> shapefile::Point {
        return shapefile::Point::new(
            point.x*self.x_scale + self.x_shift,
            point.y*self.y_scale + self.y_shift,
        );
    }
}


enum CoordinateSystem {
    YGoesUp, YGoesDown,
}


/**
 * I need to just have one type of error; I don't need to distinguish errors at all.
 * I feel like something about this is wrong or bad, but idk we'll see if it works.
 */
#[derive(Debug)]
struct MyError {
    message: String,
}

impl MyError {
    fn new(message: String) -> MyError {
        return MyError {message: message};
    }
}
