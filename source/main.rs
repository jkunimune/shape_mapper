use core::f64;
use std::{fs, iter};
use serde::Deserialize;
use shapefile::dbase::FieldValue;
use shapefile::record::EsriShape;
use shapefile::{Point, Shape};


fn main() -> () {
    let yaml = fs::read_to_string(
        "configurations/congresentatives.yml").unwrap();
    let configuration: Configuration = serde_yaml::from_str(&yaml).unwrap();
    let map_bounding_box = configuration.bounding_box.unwrap();
    let map_width = f64::abs(map_bounding_box.right - map_bounding_box.left);
    let map_height = f64::abs(map_bounding_box.bottom - map_bounding_box.top);
    let mut svg_code = format!(
        "\
<svg viewBox=\"{} {} {} {}\" width=\"{}mm\" height=\"{}mm\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">
  <title>{}</title>
  <desc>{}</desc>
  <style>
{}
  </style>
\
        ",
        map_bounding_box.left, map_bounding_box.top,
        map_width, map_height,
        map_width, map_height,
        configuration.title, configuration.description, configuration.style,
    );
    let element_index: &mut u32 = &mut 0;
    for content in configuration.content {
        svg_code.push_str(&transcribe_as_svg(content, &map_bounding_box, &configuration.region, 1, element_index).unwrap());
    }
    svg_code.push_str("</svg>\n");

    let image_filename = "maps/congresentatives.svg";
    fs::create_dir_all("maps/").unwrap();
    fs::write(image_filename, svg_code).unwrap();
}


fn transcribe_as_svg(content: Content, outer_bounding_box: &Box, outer_region: &Option<Box>, indent_level: usize, element_count: &mut u32) -> Result<String, String> {
    let indentation: String = iter::repeat("  ").take(indent_level).collect();
    let mut string = String::new();
    match content {
        Content::Group{ class, content: sub_contents, bounding_box: sub_bounding_box, region: sub_region } => {
            let bounding_box = match &sub_bounding_box {
                Some(sub_bounding_box) => sub_bounding_box,
                None => outer_bounding_box,
            };
            let region = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            let group_index = *element_count;
            string.push_str(&format!("{}<clipPath id=\"clip_path_{}\">\n", &indentation, group_index));
            string.push_str(&format!(
                    "{}  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" id=\"rect_{}\"/>\n",
                    &indentation, bounding_box.left, bounding_box.top,
                    bounding_box.right - bounding_box.left,
                    bounding_box.bottom - bounding_box.top, group_index,
            ));
            string.push_str(&format!("{}</clipPath>\n", &indentation));
            string.push_str(&match class {
                Some(class) => format!("{}<g clip-path=\"url(#clip_path_{})\" class=\"{}\">\n", &indentation, group_index, &class),
                None        => format!("{}<g clip-path=\"url(#clip_path_{})\">\n", &indentation, group_index),
            });
            string.push_str(&format!("{}  <use href=\"#rect_{}\" class=\"background\"/>\n", &indentation, group_index));
            for sub_content in sub_contents {
                string.push_str(&transcribe_as_svg(sub_content, bounding_box, region, indent_level + 1, element_count)?);
            }
            string.push_str(&format!("{}  <use href=\"#rect_{}\" class=\"map_border\"/>\n", &indentation, group_index));
            string.push_str(&format!("{}</g>\n", &indentation));
            *element_count += 1;
        }
        Content::Layer{ filename, class, class_column, self_clip, filters } => {
            let bounding_box = outer_bounding_box;
            let region = outer_region.as_ref().ok_or(format!(
                "every layer must have a region defined somewhere in its hierarchy, but '{}' does not.", filename))?;
            let transform = Transform::between(region, bounding_box);
            string.push_str(&match class {
                Some(class) => format!("{}<g class=\"{}\">\n", &indentation, &class),
                None        => format!("{}<g>\n", &indentation),
            });
            let mut reader = shapefile::Reader::from_path(
                format!("data/{}.shp", filename)).map_err(|err| err.to_string())?;
            'shape_loop:
            for shape_record in reader.iter_shapes_and_records() {
                let shape_record = shape_record.unwrap();
                let (shape, record) = shape_record;
                // first, discard anything that contradicts a filter
                match &filters {
                    Some(filters) => {
                        for Filter {key, valid_values} in filters {
                            let valid: bool = match record.get(&key).ok_or(format!("you can't filter on the field '{}' because it doesn't exist.", key))? {
                                FieldValue::Numeric(Some(number)) => valid_values.contains(&f64::to_string(number)),
                                FieldValue::Numeric(None) => false,
                                FieldValue::Character(Some(characters)) => valid_values.contains(characters),
                                FieldValue::Character(None) => false,
                                _ => return Err(String::from("you can only filter on numerical and character fields right now.")),
                            };
                            if !valid {
                                continue 'shape_loop;
                            }
                        }
                    }
                    None => {}
                }
                // also discard it if its bounding box doesn't intersect the desired region
                if !any_of_shape_is_in_box(&shape, &region) {
                    continue;
                }
                // convert it to a d string
                let shape_string = match shape {
                    Shape::Polygon(polygon) => {
                        let mut rings_as_nested_vec: Vec<Vec<Point>> = Vec::with_capacity(polygon.rings().len());
                        for i in 0..polygon.rings().len() {
                            rings_as_nested_vec.push(polygon.rings()[i].points().to_vec());
                        }
                        convert_points_to_path_string(&rings_as_nested_vec, true, &transform)
                    }
                    Shape::Polyline(polyline) => {
                        convert_points_to_path_string(polyline.parts(), false, &transform)
                    }
                    _ => {
                        panic!("we only do polygons and polylines right now.");
                    }
                };
                let sub_class = match &class_column {
                    Some(class_column) => match record.get(class_column) {
                        Some(value) => match value {
                            FieldValue::Character(characters) => match characters {
                                Some(characters) => Some(characters.to_lowercase()),
                                None => None,
                            },
                            FieldValue::Numeric(number) => match number {
                                Some(number) => Some(format!("{}_{}", class_column.to_lowercase(), number)),
                                None => None,
                            },
                            _ => return Err(String::from("I don't know how to print this field type.")),
                        },
                        None => return Err(format!("there doesn't seem to be a '{}' collum in '{}.shp'.", class_column, &filename)),
                    }
                    None => None,
                };
                let shape_string = match sub_class {
                    Some(sub_class) => format!("{} class=\"{}\"", shape_string, sub_class),
                    None => shape_string,
                };
                let shape_index = *element_count;
                let shape_string = match self_clip {
                    Some(true) => {
                        format!("{}  <clipPath id=\"clip_path_{}\">\n", &indentation, shape_index) + &
                        format!("{}    {} id=\"shape_{}\"/>\n", &indentation, &shape_string, shape_index) + &
                        format!("{}  </clipPath>\n", &indentation) + &
                        format!(
                            "{}  <use href=\"#shape_{}\" style=\"clip-path: url(#clip_path_{})\"/>\n",
                            &indentation, shape_index, shape_index
                        )
                    }
                    _ => {
                        format!("{}  {}/>\n", &indentation, &shape_string)
                    }
                };
                string.push_str(&shape_string);
                *element_count += 1;
            }
            string.push_str(&format!("{}</g>\n", &indentation));
        }
    }
    return Ok(string);
}


fn convert_points_to_path_string(sections: &Vec<Vec<Point>>, close_path: bool, transform: &Transform) -> String {
    let mut path_string = String::new();
    for section in sections {
        for (i, point) in section.iter().enumerate() {
            let point = Transform::apply(transform, &point);
            let segment_string = if i == 0 {
                &format!("M{:.2},{:.2} ", point.x, point.y)
            }
            else if i == section.len() - 1 && close_path {
                "Z"
            }
            else {
                &format!("L{:.2},{:.2} ", point.x, point.y)
            };
            path_string.push_str(segment_string);
        }
    }
    format!("<path d=\"{}\"", path_string)
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
    description: String,
    style: String,
    bounding_box: Option<Box>,
    region: Option<Box>,
    content: Vec<Content>,
}


#[derive(Deserialize)]
enum Content {
    /// some geographical data loaded from a shapefile
    Layer {
        /// the shapefile name containing the data, without the 'data/' or '.shp'. */
        filename: String,
        /// the SVG class to add to the shapes in this layer, if any
        class: Option<String>,
        /// the record field key to use to tag each shape with a unique class, if such tags are desired.
        class_column: Option<String>,
        /// whether to make this shape's strokes be confined within its shape
        self_clip: Option<bool>,
        /// key-[value] pairs used to show only a subset of the shapes in the file
        filters: Option<Vec<Filter>>,
    },
    /// a group of other Contents
    Group {
        /// the things to put inside this group
        content: Vec<Content>,
        /// the SVG class to add to the elements in this group, if any
        class: Option<String>,
        /// the box to which to fit this group's content
        bounding_box: Option<Box>,
        /// the geographical region to include. shapes wholly outside this region will be discarded,
        /// and the content will be scaled to fit this region to the bounding box.
        region: Option<Box>,
    },
}


#[derive(Deserialize)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}


#[derive(Deserialize)]
struct Filter {
    key: String,
    valid_values: Vec<String>,
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

    fn apply(self: &Transform, input: &Point) -> Point {
        return Point::new(
            input.x*self.x_scale + self.x_shift,
            input.y*self.y_scale + self.y_shift,
        );
    }
}
