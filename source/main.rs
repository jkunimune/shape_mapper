use core::f64;
use regex::Regex;
use std::{env, fs, iter};
use serde::Deserialize;
use shapefile::dbase::{FieldValue, Record};
use shapefile::record::EsriShape;
use shapefile::Shape;



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
<svg viewBox=\"{:.2} {:.2} {:.2} {:.2}\" width=\"{:.2}mm\" height=\"{:.2}mm\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">
  <title>{}</title>
  <desc>{}</desc>
  <style>
{}
  </style>
\
        ",
        map_bounding_box.left, map_bounding_box.top, map_width, map_height,
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
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"",
                bounding_box.left, bounding_box.top,
                bounding_box.right - bounding_box.left,
                bounding_box.bottom - bounding_box.top);
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
                "{}<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\"{}/>\n",
                &indentation, start.x, start.y, end.x, end.y, class_string));
        },

        // for a rectangle, use a <rect>
        Content::Rectangle { coordinates, .. } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            string.push_str(&format!(
                "{}<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"{}/>\n",
                &indentation,
                coordinates.left*transform.x_scale + transform.x_shift,
                coordinates.top*transform.y_scale + transform.y_shift,
                (coordinates.right - coordinates.left)*transform.x_scale,
                (coordinates.bottom - coordinates.top)*transform.y_scale,
                &class_string));
        }

        // for a label, use a <text> element
        Content::Label { text, coordinates, .. } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            let coordinates = Transform::apply(&transform, &coordinates.to_shapefile_point());

            string.push_str(&format!(
                "{}<text x=\"{:.2}\" y=\"{:.2}\"{}>{}</text>\n",
                &indentation, coordinates.x, coordinates.y, &class_string, &text));
        },

        // for a layer, put down a <g> containing a bunch of <path>s or whatever
        Content::Layer{
                filename, class_column, label_column, double, self_clip, filters, marker, marker_size, class: _
        } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);

            string.push_str(&format!("{}<g{}>\n", &indentation, &class_string));
            let mut reader = shapefile::Reader::from_path(
                format!("data/{}.shp", filename)).or(Err(format!("could not find `data/{}.dbf`", &filename)))?;

            let scale_string = format!("1:{:.0}", 1e3/f64::min(transform.x_scale, -transform.y_scale));
            let shape_count = reader.shape_count().map_err(|err| err.to_string())?;
            println!("plotting {} shapes from `{}.shp` at scale {}", shape_count, filename, scale_string);

            for shape_record in reader.iter_shapes_and_records() {
                let shape_record = shape_record.unwrap();
                let (shape, record) = shape_record;

                // first, discard anything that contradicts a filter
                match &filters {
                    Some(filters) => {
                        if !matches(filters, &record)? {
                            continue;
                        }
                    }
                    None => {}
                }
                // also discard it if its bounding box doesn't intersect the desired region
                if !any_of_shape_is_in_box(&shape, &region) {
                    continue;
                }

                // start by converting the shape to an SVG string of some kind
                let shape_string: String = match &marker {
                    // if marker was specified, put a marker at the center of each shape
                    Some(marker_filename) => {
                        // make sure we have a marker
                        let marker_size = match marker_size {
                            Some(number) => number,
                            None => {
                                return Err(format!("the `{}.shp` layer has a marker, but no marker size is given.", filename));
                            }
                        };
                        // check for any incompatible options
                        match self_clip {
                            Some(true) => {
                                return Err(String::from("the `self_clip` option is incompatible with the `marker` option."));
                            }
                            _ => {}
                        }
                        let marker_location = Transform::apply(&transform, &center_of(&shape)?);
                        position_and_scale_marker(marker_filename, &marker_location, marker_size)?
                    }
                    // if not, draw out the size and shape of each shape
                    None => {
                        // check for any incompatible options
                        match marker_size {
                            Some(_) => {
                                return Err(String::from("you may not use the `marker_size` option without the `marker` option."));
                            }
                            None => {}
                        }
                        // draw it however you draw this shape
                        match shape {
                            Shape::Polygon(polygon) => {
                                // combine the rings of the polygon into a Vec<Vec<Point>>
                                let mut rings_as_nested_vec: Vec<Vec<shapefile::Point>> = Vec::with_capacity(polygon.rings().len());
                                for i in 0..polygon.rings().len() {
                                    rings_as_nested_vec.push(polygon.rings()[i].points().to_vec());
                                }
                                // convert it to a d string and make it a path
                                convert_points_to_path_string(&rings_as_nested_vec, true, &transform)
                            }
                            Shape::Polyline(polyline) => {
                                // convert it to a d string and make a path
                                convert_points_to_path_string(polyline.parts(), false, &transform)
                            }
                            Shape::Point(_) => {
                                return Err(format!("data/{}.shp is a POINT shapefile.  POINT layers must always have a `marker`.", filename))
                            }
                            _ => {
                                return Err(format!("we don't support {} shapefiles right now.", shape.shapetype().to_string()));
                            }
                        }
                    }
                };

                // tack on the class, if there is one
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
                    Some(sub_class) => insert_attribute(&shape_string, "class", &sub_class)?,
                    None => shape_string,
                };

                // nest it in a clipPath element, if desired
                let shape_index = *element_count;
                let shape_string = match self_clip {
                    Some(true) => {
                        format!("  <clipPath id=\"clip_path_{}\">\n", shape_index) + &
                        "  " + &shape_string + &
                        "  </clipPath>\n" + &
                        &insert_attribute(&shape_string, "style", &format!("clip-path: url(#clip_path_{})", shape_index))?
                    }
                    _ => shape_string,
                };

                // double it up, if desired
                let shape_string = match double {
                    Some(true) => {
                        insert_attribute(&shape_string, "class", "bottom")? + &
                        insert_attribute(&shape_string, "class", "top")?
                    }
                    _ => shape_string
                };

                // add the proper indentation
                let sub_indentation = String::from("  ") + &indentation;
                let shape_string = prepend_to_each_line(&shape_string, &sub_indentation);

                // add it to the shape
                string.push_str(&shape_string);
                *element_count += 1;
            }

            // now, if labels are desired, go thru and add labels on top of all the shapes
            match label_column {
                Some(label_column) => {
                    let mut reader = shapefile::Reader::from_path(
                        format!("data/{}.shp", filename)).or(Err(format!("could not find `data/{}.dbf`", &filename)))?;
                    for shape_record in reader.iter_shapes_and_records() {
                        let shape_record = shape_record.unwrap();
                        let (shape, record) = shape_record;
        
                        // first, discard anything that contradicts a filter
                        match &filters {
                            Some(filters) => {
                                if !matches(filters, &record)? {
                                    continue;
                                }
                            }
                            None => {}
                        }
                        // also discard it if its bounding box doesn't intersect the desired region
                        if !any_of_shape_is_in_box(&shape, &region) {
                            continue;
                        }

                        // decide what the label should say
                        let text = match record.get(&label_column) {
                            Some(FieldValue::Character(characters)) => match characters {
                                Some(characters) => characters,
                                None => continue,
                            },
                            _ => {
                                return Err(String::from("you can only label by string columns"));
                            }
                        };
                        // decide where to put the label
                        let location = Transform::apply(&transform, &center_of(&shape)?);
                        // add the label to the string
                        string.push_str(&format!(
                            "{}  <text class=\"label\" x=\"{:.2}\" y=\"{:.2}\">{}</text>\n",
                            &indentation, location.x, location.y, text));
                    }
                }
                _ => {}
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
    format!("<path d=\"{}\"/>\n", path_string)
}


fn position_and_scale_marker(marker_filename: &String, marker_location: &shapefile::Point, marker_size: f64) -> Result<String, String> {
    let marker_svg = fs::read_to_string(format!("markers/{}.svg", marker_filename)).map_err(|err| err.to_string())?;
    let regex_match = Regex::new(r"<svg[^>]*>\s*(<[^>]+/>\n)\s*</svg>").unwrap().captures(&marker_svg);
    let marker_string = regex_match.ok_or(format!("markers/{}.svg was malformed", marker_filename))?.get(1).ok_or(String::from("bruh"))?.as_str();
    return insert_attribute(
        marker_string, "transform",
        &format!(
            "translate({}, {}) scale({})",
            marker_location.x, marker_location.y, f64::sqrt(marker_size)));
}


fn matches(filters: &Vec<Filter>, record: &Record) -> Result<bool, String> {
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
            return Ok(false);
        }
    }
    return Ok(true);
}


fn center_of(shape: &Shape) -> Result<shapefile::Point, String> {
    return match bounds_of(shape) {
        Ok([x_range, y_range]) => {
            Ok(shapefile::Point { x: (x_range[0] + x_range[1])/2., y: (y_range[0] + y_range[1])/2. })
        }
        Err(_) => {
            Err(String::from("I cannot calculate the center of this shape because it has no geometry."))
        }
    }
}


fn any_of_shape_is_in_box(shape: &Shape, boks: &Box) -> bool {
    return match bounds_of(shape) {
        Ok([x_range, y_range]) => {
            x_range[0] < f64::max(boks.left, boks.right) &&
            x_range[1] > f64::min(boks.left, boks.right) &&
            y_range[0] < f64::max(boks.bottom, boks.top) &&
            y_range[1] > f64::min(boks.bottom, boks.top)
        }
        Err(_) => {
            return false;
        }
    }
}


/// extract the bounding box of this shape, or return Err(()) if the shape has no geometry
fn bounds_of(shape: &Shape) -> Result<[[f64; 2]; 2], ()> {
    return match shape {
        Shape::NullShape => return Err(()),
        Shape::Point(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::PointM(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::PointZ(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::Polyline(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::PolylineM(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::PolylineZ(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::Polygon(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::PolygonM(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::PolygonZ(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::Multipoint(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::MultipointM(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::MultipointZ(shape) => Ok([shape.x_range(), shape.y_range()]),
        Shape::Multipatch(shape) => Ok([shape.x_range(), shape.y_range()]),
    };
}


/// take an SVG string and insert the given attribute key and value to the last tag
/// in it.  if the key is already there, append to the existing value
fn insert_attribute(element: &str, key: &str, value: &str) -> Result<String, String> {
    let key_value_pattern = Regex::new(&format!("{}=\"([^\"]*)\"", key)).unwrap();
    match key_value_pattern.find(element) {
        Some(_) => {
            return Ok(key_value_pattern.replace(element, format!("{}=\"$1 {}\"", key, value)).into_owned());
        }
        _ => {}
    }
    let tag_pattern = Regex::new(r"<[a-z]+()[ /]").unwrap();
    let mut last_tag_index: Option<usize> = None;
    for capture in tag_pattern.captures_iter(element) {
        last_tag_index = Some(capture.get(1).unwrap().start());
    };
    match last_tag_index {
        Some(last_tag_index) => {
            return Ok(format!(
                "{} {}=\"{}\"{}",
                &element[..last_tag_index], key, value, &element[last_tag_index..]))
        }
        None => {
            return Err(format!("I couldn't find any tags in the string '{}'", element))
        }
    }
}


/// break the string up at newlines and add the given string to the start of each section
fn prepend_to_each_line(string: &str, prefix: &str) -> String {
    let mut new_string = String::new();
    for part in Regex::new("\n").unwrap().split(string) {
        new_string.push_str(prefix);
        new_string.push_str(part);
    }
    return new_string;
}


/// replace problematic characters like , to _ and make it all lowercase
fn sanitize(string: &str) -> String {
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
        /// the record field key to put in the label that gets added to each shape, if such labels are desired.
        label_column: Option<String>,
        /// the name of the SVG (without the 'markers/' or '.svg') to put at the center of this thing
        marker: Option<String>,
        /// the desired area of the SVG in square millimeters
        marker_size: Option<f64>,
        /// whether to duplicate each shape in this thing
        double: Option<bool>,
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
