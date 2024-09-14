use core::f64;
use regex::Regex;
use std::{env, fs, iter};
use serde::Deserialize;
use shapefile::dbase::FieldValue;
use shapefile::record::EsriShape;
use shapefile::Shape;


/// the length of one SVG unit in mm
static POINT: f64 = 0.352778;


fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if args.len() > 2 {
        return Err(String::from("you must pass only one argument, representing the filename of the configuration file without the 'yml'"));
    }
    let filename = args.get(1).ok_or(String::from("Please pass the filename of the configuration file minus the 'yml' as a command line argument."))?;

    let yaml = fs::read_to_string(
        format!("configurations/{}.yml", filename)).map_err(|err| err.to_string())?;
    let configuration: Configuration = serde_yaml::from_str(&yaml).map_err(|err| err.to_string())?;

    println!("generating a map of '{}' based on `configurations/{}.yml`.", configuration.title, filename);

    let map_bounding_box = configuration.bounding_box.ok_or("the top level must have a bounding box")?;
    let map_width = f64::abs(map_bounding_box.right - map_bounding_box.left);
    let map_height = f64::abs(map_bounding_box.bottom - map_bounding_box.top);
    let mut svg_code = format!(
        "\
<svg viewBox=\"{:.1} {:.1} {:.1} {:.1}\" width=\"{:.1}mm\" height=\"{:.1}mm\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">
  <title>{}</title>
  <desc>{}</desc>
  <style>
{}
  </style>
\
        ",
        map_bounding_box.left/POINT, map_bounding_box.top/POINT,
        map_width/POINT, map_height/POINT,
        map_width, map_height,
        configuration.title, configuration.description, configuration.style,
    );

    let element_index: &mut u32 = &mut 0;
    for content in configuration.content {
        svg_code.push_str(&transcribe_as_svg(content, &map_bounding_box, &configuration.region, 1, element_index)?);
    }

    svg_code.push_str("</svg>\n");

    println!("saving `maps/{}.svg.`", filename);

    fs::create_dir_all("maps/").map_err(|err| err.to_string())?;
    fs::write(format!("maps/{}.svg", filename), svg_code).map_err(|err| err.to_string())?;

    println!("done!");
    return Ok(());
}


fn transcribe_as_svg(content: Content, outer_bounding_box: &Box, outer_region: &Option<Box>, indent_level: usize, element_count: &mut u32) -> Result<String, String> {
    // decide how much indentation it will have
    let indentation: String = iter::repeat("  ").take(indent_level).collect();
    // prepare to add a class if there is any
    let class_string = match content.get_class() {
        Some(class) => format!(" class=\"{}\"", sanitize(class)),
        None => String::from(""),
    };

    // everything after that depends on what kind of content it is...
    let mut string = String::new();
    match content {

        // for a group, put down a <rect>, a <g> with whatever the sub-contents are, and then another <rect>
        Content::Group{ content: sub_contents, bounding_box: sub_bounding_box, region: sub_region, frame, .. } => {
            // this group may override the outer bounding box and region
            let bounding_box = match &sub_bounding_box {
                Some(sub_bounding_box) => sub_bounding_box,
                None => outer_bounding_box,
            };
            let region = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            let frame = frame.unwrap_or(false);
            let group_index = *element_count;
            // write all the stuff
            let rect_string = format!(
                "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\"",
                bounding_box.left/POINT, bounding_box.top/POINT,
                (bounding_box.right - bounding_box.left)/POINT,
                (bounding_box.bottom - bounding_box.top)/POINT);
            string.push_str(&format!("{}<clipPath id=\"clip_path_{}\">\n", &indentation, group_index));
            string.push_str(&format!("{}  {}/>\n", &indentation, &rect_string));
            string.push_str(&format!("{}</clipPath>\n", &indentation));
            string.push_str(&format!("{}<g clip-path=\"url(#clip_path_{})\"{}>\n", &indentation, group_index, &class_string));
            if frame {
                string.push_str(&format!("{}  {} class=\"background\"/>\n", &indentation, &rect_string));
            }
            for sub_content in sub_contents {
                string.push_str(&transcribe_as_svg(sub_content, bounding_box, region, indent_level + 1, element_count)?);
            }
            if frame {
                string.push_str(&format!("{}  {} class=\"frame\"/>\n", &indentation, &rect_string));
            }
            string.push_str(&format!("{}</g>\n", &indentation));
        }

        // for a line, jou just need a single <line>
        Content::Line { start, end, .. } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            let start = Transform::apply(&transform, &start.to_shapefile_point());
            let end = Transform::apply(&transform, &end.to_shapefile_point());
            string.push_str(&format!(
                "{}<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\"{}/>/n",
                &indentation, start.x/POINT, start.y/POINT, end.x/POINT, end.y/POINT, class_string));
        },

        // for a rectangle, use a <rect>
        Content::Rectangle { coordinates, .. } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            string.push_str(&format!(
                "{}<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\"{}/>\n",
                &indentation,
                (coordinates.left*transform.x_scale + transform.x_shift)/POINT,
                (coordinates.top*transform.y_scale + transform.y_shift)/POINT,
                (coordinates.right - coordinates.left)*transform.x_scale/POINT,
                (coordinates.bottom - coordinates.top)*transform.y_scale/POINT,
                &class_string));
        }

        // for a label, use a <text> element
        Content::Label { text, coordinates, .. } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            let coordinates = Transform::apply(&transform, &coordinates.to_shapefile_point());

            string.push_str(&format!(
                "{}<text x=\"{:.1}\" y=\"{:.1}\"{}>{}</text>\n",
                &indentation, coordinates.x/POINT, coordinates.y/POINT, &class_string, &text));
        },

        // for a layer, put down a <g> containing a bunch of <path>s or whatever
        Content::Layer{ filename, class_column, self_clip, filters, .. } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            string.push_str(&format!("{}<g{}>\n", &indentation, &class_string));
            let mut reader = shapefile::Reader::from_path(
                format!("data/{}.shp", filename)).or(Err(format!("could not find `data/{}.dbf`", &filename)))?;

            let scale_string = format!("1:{:.0}", 1e3/f64::min(transform.x_scale, -transform.y_scale));
            let shape_count = reader.shape_count().map_err(|err| err.to_string())?;
            println!("plotting {} shapes from `{}.shp` at scale {}", shape_count, filename, scale_string);
    
            'shape_loop:
            for shape_record in reader.iter_shapes_and_records() {
                let shape_record = shape_record.unwrap();
                let (shape, record) = shape_record;
                // first, discard anything that contradicts a filter
                match &filters {
                    Some(filters) => {
                        for filter in filters {
                            let valid = match filter {
                                Filter::OneOf { key, valid_values } => {
                                    let value = record.get(&key).ok_or(format!("you can't filter on the field '{}' because it doesn't exist.", key))?;
                                    match value {
                                        FieldValue::Numeric(Some(number)) => valid_values.contains(&f64::to_string(number)),
                                        FieldValue::Numeric(None) => false,
                                        FieldValue::Character(Some(characters)) => valid_values.contains(characters),
                                        FieldValue::Character(None) => false,
                                        _ => return Err(String::from("you can only filter on numerical and character fields right now.")),
                                    }
                                }
                                Filter::GreaterThan { key, cutoff } => {
                                    let value = record.get(&key).ok_or(format!("you can't filter on the field '{}' because it doesn't exist.", key))?;
                                    match value {
                                        FieldValue::Float(Some(number)) => number > cutoff,
                                        FieldValue::Float(None) => false,
                                        _ => return Err(String::from("GreaterThan filters must act on float32 fields.")),
                                    }
                                }
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
                        let mut rings_as_nested_vec: Vec<Vec<shapefile::Point>> = Vec::with_capacity(polygon.rings().len());
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
                                Some(characters) => Some(sanitize(characters).replace(" ", "_")),
                                None => None,
                            },
                            FieldValue::Numeric(number) => match number {
                                Some(number) => Some(format!("{}_{}", sanitize(class_column), number)),
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
                            "{}  {} style=\"clip-path: url(#clip_path_{})\"/>\n",
                            &indentation, &shape_string, shape_index
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

    *element_count += 1;
    return Ok(string);
}


fn convert_points_to_path_string(sections: &Vec<Vec<shapefile::Point>>, close_path: bool, transform: &Transform) -> String {
    let mut path_string = String::new();
    for section in sections {
        for (i, point) in section.iter().enumerate() {
            let point = Transform::apply(transform, &point);
            let segment_string = if i == 0 {
                &format!("M{:.1},{:.1} ", point.x/POINT, point.y/POINT)
            }
            else if i == section.len() - 1 && close_path {
                "Z"
            }
            else {
                &format!("L{:.1},{:.1} ", point.x/POINT, point.y/POINT)
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


fn sanitize(string: &String) -> String {
    return Regex::new(r"[{},.:;]").unwrap().replace_all(&string.to_lowercase(), "_").into_owned();
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
    /// a line segment from one location to another
    Line {
        /// one endpoit of the line
        start: SerializablePoint,
        /// the other endpoint of the line
        end: SerializablePoint,
        /// the SVG class to add to the element, if any
        class: Option<String>,
    },
    /// a plain box over some geographic area
    Rectangle {
        /// the boundaries of the rectangle
        coordinates: Box,
        /// the SVG class to add to the element, if any
        class: Option<String>,
    },
    /// a bit of text
    Label {
        /// the characters to write
        text: String,
        /// the location to put them
        coordinates: SerializablePoint,
        /// the SVG class to add to the element
        class: Option<String>,
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
        /// whether to add a rect for a background and frame
        frame: Option<bool>,
    },
}

impl Content {
    fn get_class(self: &Content) -> &Option<String> {
        return match self {
            Content::Layer { class, .. } => class,
            Content::Line { class, .. } => class,
            Content::Rectangle { class, .. } => class,
            Content::Label { class, .. } => class,
            Content::Group { class, .. } => class,
        };
    }
}


#[derive(Deserialize)]
struct SerializablePoint {
    x: f64,
    y: f64,
}

impl SerializablePoint {
    fn to_shapefile_point(self: &SerializablePoint) -> shapefile::Point {
        return shapefile::Point { x: self.x, y: self.y };
    }
}


#[derive(Deserialize)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}


#[derive(Deserialize)]
enum Filter {
    OneOf {
        key: String,
        valid_values: Vec<String>,
    },
    GreaterThan {
        key: String,
        cutoff: f32,
    }
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

    fn apply(self: &Transform, input: &shapefile::Point) -> shapefile::Point {
        return shapefile::Point::new(
            input.x*self.x_scale + self.x_shift,
            input.y*self.y_scale + self.y_shift,
        );
    }
}
