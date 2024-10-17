#![allow(non_snake_case)]

use core::f64;
use regex::Regex;
use std::{env, fs};
use serde::Deserialize;
use shapefile::dbase::{FieldValue, Record};
use shapefile::record::EsriShape;
use shapefile::Shape;



const EARTH_RADIUS: f64 = 6.371e9; // mm (TODO: infer this from the .prj file?)
const CURVE_PRECISION: f64 = 1.0; // mm



fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if args.len() > 2 {
        return Err(String::from("you must pass only one argument, representing the filename of the configuration file without the 'yml'"));
    }
    let filename = args.get(1).ok_or(String::from("Please pass the filename of the configuration file minus the 'yml' as a command line argument."))?;

    let file = fs::read_to_string(format!("configurations/{}.yml", filename));
    let yaml = match file {
        Ok(yaml) => yaml,
        Err(message) => return Err(format!("I could not read 'configurations/{}.yml' because {}", filename, message)),
    };
    let configuration: Configuration = serde_yaml::from_str(&yaml).map_err(|err| err.to_string())?;

    println!("generating a map of '{}' based on `configurations/{}.yml`.", configuration.title, filename);

    let map_bounding_box = configuration.bounding_box.ok_or("the top level must have a bounding box")?;
    let top_level_transform = match configuration.transform {
        Some(..) => configuration.transform,
        None => match &configuration.region {
            Some(region) => Some(Transform::between(region, &map_bounding_box)),
            None => None,
        }
    };
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
    for content in configuration.contents {
        let content = load_content(content, &configuration.region)?;
        let content = transform_content(content, &top_level_transform)?;
        let content = transcribe_content_as_svg(content, &map_bounding_box, &configuration.region, element_index)?;
        let content = prepend_to_each_line(&content, "  ");
        svg_code.push_str(&content);
    }

    svg_code.push_str("</svg>\n");

    println!("saving `maps/{}.svg.`", filename);

    fs::create_dir_all("maps/").map_err(|err| err.to_string())?;
    fs::write(format!("maps/{}.svg", filename), svg_code).map_err(|err| err.to_string())?;

    println!("done!");
    return Ok(());
}


/// return a copy of this content that is the same except that any Layers and Graticules
/// are resolved into groups of Paths
fn load_content(content: Content, outer_region: &Option<Box>) -> Result<Content, String> {
    match content {

        // for a Group, load its children recursively
        Content::Group { contents: sub_contents, bounding_box, region: sub_region, transform, clip, frame, class } => {
            let region: &Option<Box> = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            let mut loaded_sub_contents = Vec::with_capacity(sub_contents.len());
            for sub_content in sub_contents {
                loaded_sub_contents.push(load_content(sub_content, region)?);
            }
            return Ok(Content::Group {
                contents: loaded_sub_contents,
                region: sub_region,
                bounding_box, transform,
                class, clip, frame,
            });
        }

        // resolve a Layer by loading geographic data from disc and making a bunch of Paths or Markers
        Content::Layer { filename, class, class_column, label_column, label_case, marker, marker_size, double, self_clip, filters } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            // check for incompatible options
            if marker.is_none() && marker_size.is_some() {
                return Err(String::from("you may not use the `marker_size` option without the `marker` option."));
            }
            if marker.as_ref().is_some_and(|value| value != "none") && self_clip == Some(true) {
                return Err(String::from("the `self_clip` option is incompatible with the `marker` option."));
            }

            let mut contents = Vec::new();
            let mut labels = Vec::new();
            let mut reader = shapefile::Reader::from_path(
                format!("data/{}.shp", filename)).or(Err(format!("could not find `data/{}.dbf`", &filename)))?;

            let marker_data = match &marker {
                // if marker was unspecified, don't load anything
                None => None,
                // if marker was explicitly "none", don't load anything
                Some(marker) if marker == "none" => None,
                // if marker was specified, load its content from disc and unwrap marker_size
                Some(marker_filename) => {
                    // make sure we have a marker size
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
                    Some((load_SVG(&marker_filename)?, marker_size))
                }
            };

            'shape_loop:
            for shape_record in reader.iter_shapes_and_records() {
                let shape_record = shape_record.unwrap();
                let (shape, record) = shape_record;

                // first, discard anything that contradicts a filter
                match &filters {
                    Some(filters) => {
                        for filter in filters {
                            if !filter.matches(&record)? {
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

                // determine if we should add a class to this
                let shape_class = match &class_column {
                    Some(class_column) => match record.get(class_column) {
                        Some(value) => match value {
                            FieldValue::Character(characters) => match characters {
                                Some(characters) => Some(sanitize_CSS(characters).replace(" ", "_")),
                                None => None,
                            },
                            FieldValue::Numeric(number) => match number {
                                Some(number) => Some(format!("{}_{}", sanitize_CSS(class_column), number)),
                                None => None,
                            },
                            _ => return Err(String::from("I don't know how to print this field type.")),
                        },
                        None => return Err(format!("there doesn't seem to be a '{}' collum in '{}.shp'.", class_column, &filename)),
                    }
                    None => None,
                };

                let location = center_of(&shape)?;

                // convert the shape to a Path or a Marker
                let shape = if marker.is_none() {
                    // if no marker was specified, try to make a Content::Path
                    // unwrap self_clip
                    let self_clip = self_clip.unwrap_or(false);
                    // draw it however you draw this shape
                    let (parts, closed) = match shape {
                        Shape::Polygon(polygon) => {
                            // combine the rings of the polygon into a Vec<Vec<Point>>
                            let mut rings_as_nested_vec: Vec<Vec<shapefile::Point>> = Vec::with_capacity(polygon.rings().len());
                            for i in 0..polygon.rings().len() {
                                rings_as_nested_vec.push(polygon.rings()[i].points().to_vec());
                            }
                            (SerializablePoint::deep_convert(&rings_as_nested_vec), true)
                        }
                        Shape::Polyline(polyline) => {
                            (SerializablePoint::deep_convert(polyline.parts()), false)
                        }
                        Shape::Point(_) => {
                            return Err(format!("data/{}.shp is a POINT shapefile.  POINT layers must always have a `marker`.", filename))
                        }
                        _ => {
                            return Err(format!("we don't support {} shapefiles right now.", shape.shapetype().to_string()));
                        }
                    };
                    // pull it all together as a Content::Path
                    Some(Content::Path {
                        parts, closed, self_clip,
                        class: shape_class,
                    })
                }
                else {
                    match &marker_data {
                        // if marker was specified, make a Content::Marker
                        Some((marker_detail, marker_size)) => {
                            let marker_location = center_of(&shape)?;
                            Some(Content::Marker {
                                detail: marker_detail.clone(),
                                location: marker_location,
                                size: *marker_size,
                                class: shape_class,
                            })
                        }
                        // if marker was specified as "none", don't do anything
                        None => {
                            None
                        }
                    }
                };

                // append it to the list, twice if so desired
                match shape {
                    Some(shape) => match double {
                        Some(true) => {
                            for position in ["bottom", "top"] {
                                let compound_class = match shape.get_class() {
                                    Some(class) => Some(String::from(class) + " " + position),
                                    None => Some(String::from(position)),
                                };
                                let shape = shape.clone().set_class(compound_class);
                                contents.push(shape);
                            }
                        }
                        _ => {
                            contents.push(shape);
                        }
                    }
                    None => {}
                }

                // add the corresponding label, if desired
                match &label_column {
                    Some(label_column) => {
                        // decide what the label should say
                        let text = match record.get(label_column) {
                            Some(FieldValue::Character(characters)) => match characters {
                                Some(characters) => characters,
                                None => continue,
                            },
                            Some(value) => {
                                return Err(format!("you can only label by string columns, but '{}' is a {} column.", label_column, value.field_type().to_string()));
                            }
                            None => {
                                return Err(format!("you can't label by the column '{}' because it doesn't seem to exist.", label_column));
                            }
                        };
                        // modify the case if desired
                        let text = match label_case {
                            Some(Case::Upper) => text.to_uppercase(),
                            Some(Case::Lower) => text.to_lowercase(),
                            Some(Case::Sentence) => text[..1].to_uppercase() + &text[1..].to_lowercase(),
                            None => text.to_owned(),
                        };
                        // add necessary escape sequences (make sure you do this after setting the case)
                        let text = sanitize_XML(&text);
                        // decide where to put the label
                        let location = coerce_in_box(&location, region);
                        labels.push(Content::Label {
                            text, location,
                            class: Some(String::from("label")),
                        });
                    }
                    _ => {}
                }
            }

            contents.append(&mut labels);

            return Ok(Content::Group {
                contents,
                class: class,
                bounding_box: None,
                region: None,
                transform: None,
                clip: Some(false),
                frame: Some(false),
            });
        }

        // resolve a Graticule by generating an array of lines
        Content::Graticule { parallel_spacing, meridian_spacing, class } => {
            let region = match outer_region {
                Some(region) => region,
                None => return Err(String::from(
                    "every layer must have a region defined somewhere in its hierarchy.")),
            };
            let mut shapes = Vec::new();

            let south = f64::ceil(region.bottom/parallel_spacing) as i32;
            let north = f64::floor(region.top/parallel_spacing) as i32;
            let west = f64::ceil(region.left/meridian_spacing) as i32;
            let east = f64::floor(region.right/meridian_spacing) as i32;
            for i in south..north + 1 {
                let latitude = (i as f64)*parallel_spacing;
                let start = SerializablePoint { x: region.left, y: latitude };
                let end = SerializablePoint { x: region.right, y: latitude };
                shapes.push(Content::Path {
                    parts: vec![vec![start, end]],
                    closed: false,
                    self_clip: false,
                    class: None
                });
            }
            for j in west..east + 1 {
                let longitude = (j as f64)*meridian_spacing;
                let start = SerializablePoint { x: longitude, y: region.bottom };
                let end = SerializablePoint { x: longitude, y: region.top };
                shapes.push(Content::Path {
                    parts: vec![vec![start, end]],
                    closed: false,
                    self_clip: false,
                    class: None
                })
            }

            return Ok(Content::Group {
                contents: shapes,
                class: class,
                bounding_box: None,
                region: None,
                transform: None,
                clip: Some(false),
                frame: Some(false),
            });
        }

        // any other form of content can just continue on as it is
        other => Ok(other),
    }
}


/// apply all necessary coordinate transforms to the points in this box.
/// all geographic data should come out in SVG coordinates rather than in shapefile coordinates.
fn transform_content(content: Content, outer_transform: &Option<Transform>) -> Result<Content, String> {
    match content {
        Content::Group { contents: sub_contents, bounding_box, region, transform, clip, frame, class } => {
            // establish the transform
            let transform = match transform {
                // either from it being stated explicitly
                Some(..) => &transform,
                None => match (&bounding_box, &region) {
                    // or by inferring it from bounding_box and region
                    (Some(bounding_box), Some(region)) => &Some(Transform::between(region, bounding_box)),
                    // or by inheriting it from an upper level
                    _ => outer_transform,
                }
            };
            let mut transformed_contents = Vec::with_capacity(sub_contents.len());
            for sub_content in sub_contents {
                transformed_contents.push(transform_content(sub_content, transform)?);
            }
            return Ok(Content::Group {
                contents: transformed_contents,
                bounding_box, region,
                clip, frame, class,
                transform: None,
            });
        },
        Content::Line { start, end, class } => {
            let transform = outer_transform.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Line {
                start: Transform::apply(&transform, &start),
                end: Transform::apply(&transform, &end),
                class: class,
            });
        },
        Content::Rectangle { coordinates, class } => {
            let transform = outer_transform.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Rectangle {
                coordinates: Transform::apply_to_box(&transform, &coordinates)?,
                class: class,
            });
        },
        Content::Path { parts, closed, self_clip, class } => {
            let transform = outer_transform.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            let mut transformed_parts = Vec::with_capacity(parts.len());
            for part in parts {
                transformed_parts.push(Transform::apply_to_curve(&transform, &part, CURVE_PRECISION));
            }
            return Ok(Content::Path {
                parts: transformed_parts,
                closed: closed,
                self_clip: self_clip,
                class: class,
            });
        },
        Content::Marker { detail, location, size, class } => {
            let transform = outer_transform.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Marker {
                detail: detail,
                location: Transform::apply(&transform, &location),
                size: size,
                class: class,
            });
        },
        Content::Label { text, location: coordinates, class } => {
            let transform = outer_transform.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Label {
                text: text,
                location: Transform::apply(&transform, &coordinates),
                class: class,
            });
        },
        Content::Layer { .. } => {
            return Err(String::from("Layers should have been purged by now"));
        },
        Content::Graticule { .. } => {
            return Err(String::from("Graticules should have been purged by now"));
        },
    }
}


fn transcribe_content_as_svg(content: Content, outer_bounding_box: &Box, outer_region: &Option<Box>, element_count: &mut u32) -> Result<String, String> {
    let class = content.get_class().clone();

    let string = match content {

        // for a group, put down a <g> with whatever the sub-contents are
        Content::Group{ contents: sub_contents, bounding_box: sub_bounding_box, region: sub_region, clip, frame, .. } => {
            // this group may override the outer bounding box and region
            let bounding_box = match &sub_bounding_box {
                Some(sub_bounding_box) => sub_bounding_box,
                None => outer_bounding_box,
            };
            let region = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            // convert the bounding box to a rectangle, as this may be useful multiple times later
            let rect_string = format!(
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"/>\n",
                bounding_box.left, bounding_box.top,
                bounding_box.right - bounding_box.left,
                bounding_box.bottom - bounding_box.top,
            );
            let group_index = *element_count;
            // write all the stuff
            let mut string = String::new();
            if clip.unwrap_or(true) {
                string.push_str(&format!("<clipPath id=\"clip_path_{}\">\n", group_index));
                string.push_str(&format!("  {}", rect_string));
                string.push_str(&format!("</clipPath>\n"));
                string.push_str(&format!("<g clip-path=\"url(#clip_path_{})\">\n", group_index));
            }
            else {
                string.push_str("<g>\n");
            }
            if frame.unwrap_or(false) {
                string.push_str(&insert_attribute(&rect_string, "class", "background")?);
            }
            for sub_content in sub_contents {
                string.push_str(&transcribe_content_as_svg(sub_content, bounding_box, region, element_count)?);
            }
            if frame.unwrap_or(false) {
                string.push_str(&insert_attribute(&rect_string, "class", "frame")?);
            }
            string.push_str(&format!("</g>\n"));
            string
        }

        // for a line, you just need a single <line>
        Content::Line { start, end, .. } =>
            format!(
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\"/>\n",
                start.x, start.y, end.x, end.y,
            ),

        // for a rectangle, use a <rect>
        Content::Rectangle { coordinates, .. } =>
            format!(
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"/>\n",
                coordinates.left, coordinates.top,
                coordinates.right - coordinates.left,
                coordinates.bottom - coordinates.top,
            ),

        // for a label, use a <text> element
        Content::Label { text, location: coordinates, .. } =>
            format!(
                "<text x=\"{:.2}\" y=\"{:.2}\">{}</text>\n",
                coordinates.x, coordinates.y, &text,
            ),

        Content::Marker { detail, location, size, .. } =>
            insert_attribute(
                &detail, "transform",
                &format!(
                    "translate({:.2}, {:.2}) scale({:.4})",
                    location.x, location.y, f64::sqrt(size),
                ),
            )?,

        Content::Path { parts, closed, self_clip, .. } => {
            // convert it to a d string and make it a path
            let mut path_string = String::new();
            for part in parts {
                for (i, point) in part.iter().enumerate() {
                    let segment_string = if i == 0 {
                        &format!("M{:.2},{:.2} ", point.x, point.y)
                    }
                    else if i == part.len() - 1 && closed {
                        "Z"
                    }
                    else {
                        &format!("L{:.2},{:.2} ", point.x, point.y)
                    };
                    path_string.push_str(segment_string);
                }
            }
            let shape_string = format!("<path d=\"{}\"/>\n", path_string);

            // nest it in a clipPath element, if desired
            let shape_index = *element_count;
            let shape_string = if self_clip {
                format!("<clipPath id=\"clip_path_{}\">\n", shape_index) + &
                shape_string + &
                "</clipPath>\n" + &
                insert_attribute(&shape_string, "style", &format!("clip-path: url(#clip_path_{})", shape_index))?
            }
            else {
                shape_string
            };
            shape_string
        }

        Content::Graticule { .. } => {
            return Err(String::from("the graticules should have all been purged by now."));    
        }
        Content::Layer{ .. } => {
            return Err(String::from("the Layers should have all been purged by now."));
        }
    };

    // tack on the class, if there is one
    let string = match class {
        Some(class) => insert_attribute(&string, "class", &sanitize_CSS(&class))?,
        None => string,
    };
    // add the proper indentation
    let string = prepend_to_each_line(&string, "  ");

    *element_count += 1;
    return Ok(string);
}


fn load_SVG(marker_filename: &String) -> Result<String, String> {
    let marker_string = fs::read_to_string(format!("markers/{}.svg", marker_filename)).or(Err(format!("couldn't read `markers/{}.svg`", marker_filename)))?;
    // extract the content from between the <svg> and </svg>
    let svg_captures = Regex::new(r"(?s)<svg[^>]*>\n(.*\n)\s*</svg>").unwrap().captures(&marker_string);
    let marker_string = match svg_captures {
        Some(svg_captures) => svg_captures.get(1).unwrap().as_str(),
        None => return Err(format!("markers/{}.svg was malformed somehow.", marker_filename)),
    };
    // extract the top-level indentation so that you can remove it
    let indentation_captures = Regex::new(r"^([ \t]*)<").unwrap().captures(&marker_string);
    let indentation = match indentation_captures {
        Some(indentation_captures) => indentation_captures.get(1).unwrap().as_str(),
        None => return Err(format!("markers/{}.svg didn't seem to start with a tag.", marker_filename)),
    };
    // remove that indentation and return
    let indentation_pattern = Regex::new(&format!("(?m)^{}", indentation)).unwrap();
    let marker_string = indentation_pattern.replace_all(marker_string, "");
    return Ok(String::from(marker_string));
}


fn center_of(shape: &Shape) -> Result<SerializablePoint, String> {
    return match bounds_of(shape) {
        Ok([x_range, y_range]) => {
            Ok(SerializablePoint { x: (x_range[0] + x_range[1])/2., y: (y_range[0] + y_range[1])/2. })
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


fn coerce_in_box(point: &SerializablePoint, boks: &Box) -> SerializablePoint {
    if boks.bottom > boks.top {
        return coerce_in_box(point, &Box {
            left: boks.left, right: boks.right, bottom: boks.top, top: boks.bottom,
        });
    }
    return SerializablePoint {
        x: f64::max(boks.left, f64::min(boks.right, point.x)),
        y: f64::max(boks.bottom, f64::min(boks.top, point.y)),
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


/// take an SVG string and insert the given attribute key and value to every top-level
/// tag in it.  if the key is already there, append to the existing value
fn insert_attribute(element: &str, key: &str, value: &str) -> Result<String, String> {
    let mut modified_element = element.to_owned();
    let mut offset = 0;

    // find the index of every top-level tag in the string
    let tag_pattern = Regex::new(r"(?m)^<[a-zA-Z]+([^>]*)>").unwrap();
    for captures in tag_pattern.captures_iter(&element) {
        let attribute_list = captures.get(1).unwrap();

        // check if the desired attribute key is already present in this tag
        let key_value_pattern = Regex::new(&format!("{}=\"([^\"]*)\"", key)).unwrap();
        // this will determine what you insert and where
        let infix = match key_value_pattern.captures_at(&element[..attribute_list.end()], attribute_list.start()) {
            // if it is present, append this value to what's already there
            Some(captures) => {
                let tag = captures.get(1).unwrap();
                Infix {content: format!(" {}", value), position: tag.end()}
            }
            // if it's not present, you have to add the whole key-value expression
            None => {
                Infix {content: format!(" {}=\"{}\"", key, value), position: attribute_list.start()}
            }
        };
        let infix = Infix { content: infix.content, position: infix.position + offset}; // don't forget that we parsed the unmodified element, but we're inserting to the modified element, and they have different lengths now
        modified_element = format!(
            "{}{}{}",
            &modified_element[..infix.position],
            infix.content,
            &modified_element[infix.position..],
        );
        offset += infix.content.len();
    }
    if offset == 0 {
        return Err(format!("I couldn't find any tags in the string '{}'", element));
    }
    else {
        return Ok(modified_element);
    }

    struct Infix {
        content: String,
        position: usize,
    }
}


/// break the string up at newlines and add the given string to the start of each section
fn prepend_to_each_line(string: &str, prefix: &str) -> String {
    let newline = Regex::new("\n").unwrap();
    let parts = newline.split(string);
    let new_parts: Vec<String> = parts.map(|part| if part.len() > 0 { format!("{}{}", prefix, part) } else { String::new() }).collect();
    return new_parts.join("\n");
}


/// replace problematic characters like , to _ and make it all lowercase
fn sanitize_CSS(string: &str) -> String {
    return Regex::new(r"[{},.:;]").unwrap().replace_all(&string.to_lowercase(), "_").into_owned();
}


/// replace problematic characters like & to their XML escape sequences
fn sanitize_XML(string: &str) -> String {
    let string = string.replace("&", "&amp;");
    let string = string.replace("<", "&lt;");
    let string = string.replace(">", "&gt;");
    let string = string.replace("'", "&apos;");
    let string = string.replace("\"", "&quot;");
    return string;
}


/// calculate the distance between a line segment and a point
fn line_point_distance(start: &SerializablePoint, end: &SerializablePoint, point: &SerializablePoint) -> f64 {
    let length2 = (end.x - start.x).powi(2) + (end.y - start.y).powi(2);
    if length2 <= 0. {
        return f64::hypot(point.x - start.x, point.y - start.y);
    }
    let t = ((point.x - start.x)*(end.x - start.x) + (point.y - start.y)*(end.y - start.y))/length2;
    let s = ((point.x - start.x)*(end.y - start.y) - (point.y - start.y)*(end.x - start.x))/length2;
    if t <= 0. {
        return f64::hypot(point.x - start.x, point.y - start.y);
    }
    else if t >= 1. {
        return f64::hypot(point.x - end.x, point.y - end.y);
    }
    else {
        return f64::abs(s)*f64::sqrt(length2);
    }
}


#[derive(Deserialize)]
struct Configuration {
    title: String,
    description: String,
    style: String,
    bounding_box: Option<Box>,
    region: Option<Box>,
    transform: Option<Transform>,
    contents: Vec<Content>,
}


#[derive(Deserialize, Clone)]
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
        /// how to set the case of the label before adding it to the image, if any such modification is desired
        label_case: Option<Case>,
        /// the name of the SVG (without the 'markers/' or '.svg') to put at the center of this thing
        marker: Option<String>,
        /// the desired area of the SVG in square millimeters
        marker_size: Option<f64>,
        /// whether to duplicate each shape in this thing (defaults to false)
        double: Option<bool>,
        /// whether to make this shape's strokes be confined within its shape (defaults to false)
        self_clip: Option<bool>,
        /// key-[value] pairs used to show only a subset of the shapes in the file
        filters: Option<Vec<Filter>>,
    },
    /// a mesh of lines of latitude and longitude
    Graticule {
        /// the spacing between each adjacent pair of lines of latitude in degrees
        parallel_spacing: f64,
        /// the spacing between each adjacent pair of lines of longitude in degrees
        meridian_spacing: f64,
        /// the SVG class to add to the element, if any
        class: Option<String>,
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
    /// a shape made of an arbitrary number of arbitrary polylines
    Path {
        /// the coordinates of the vertices in each part
        parts: Vec<Vec<SerializablePoint>>,
        /// whether each polyline should form a complete loop
        closed: bool,
        /// whether to make this shape's strokes be confined within its shape
        self_clip: bool,
        /// the SVG class to add to the element, if any
        class: Option<String>,
    },
    /// a little shape that represents a point on the map
    Marker {
        /// the SVG string that will be used verbatim to define the shape of the marker
        detail: String,
        /// the coordinates in geographical space to which the marker detail's origin will be shifted
        location: SerializablePoint,
        /// a multiplier that will be applied to the shape's area
        size: f64,
        /// the SVG class to add to the element, if any
        class: Option<String>,
    },
    /// a bit of text
    Label {
        /// the characters to write
        text: String,
        /// the location to put them
        location: SerializablePoint,
        /// the SVG class to add to the element
        class: Option<String>,
    },
    /// a group of other Contents
    Group {
        /// the things to put inside this group
        contents: Vec<Content>,
        /// the SVG class to add to the elements in this group, if any
        class: Option<String>,
        /// the box to which to crop this group's contents
        bounding_box: Option<Box>,
        /// the geographical region to include. shapes wholly outside this region will be discarded.
        region: Option<Box>,
        /// the coordinate transformation to use to map contained points from geographical coordinates
        /// to print coordinates.  defaults to an affine transform that fits the region to the bounding_box.
        transform: Option<Transform>,
        /// whether to crop the contents to the bounding box (defaults to true)
        clip: Option<bool>,
        /// whether to add a rect for a background and frame (defaults to false)
        frame: Option<bool>,
    },
}

impl Content {
    fn get_class(self: &Content) -> &Option<String> {
        return match self {
            Content::Layer { class, .. } => class,
            Content::Graticule { class, .. } => class,
            Content::Line { class, .. } => class,
            Content::Rectangle { class, .. } => class,
            Content::Label { class, .. } => class,
            Content::Path { class, .. } => class,
            Content::Marker { class, .. } => class,
            Content::Group { class, .. } => class,
        };
    }

    fn set_class(self: Content, class: Option<String>) -> Content {
        return match self {
            Content::Path { parts, closed, self_clip, .. } => Content::Path {
                class, parts: parts, closed: closed, self_clip: self_clip,
            },
            Content::Marker { detail, location, size, .. } => Content::Marker {
                class, detail: detail, location: location, size: size,
            },
            _ => panic!("this function should only be used on Paths and Markers"),
        }
    }
}


#[derive(Deserialize, Clone)]
enum Case {
    Upper,
    Lower,
    Sentence,
}


#[derive(Deserialize, Clone, PartialEq, Debug)]
struct SerializablePoint {
    x: f64,
    y: f64,
}

impl SerializablePoint {
    fn from(point: &shapefile::Point) -> SerializablePoint {
        return SerializablePoint { x: point.x, y: point.y };
    }
    fn deep_convert(points: &Vec<Vec<shapefile::Point>>) -> Vec<Vec<SerializablePoint>> {
        return points.iter().map(|ring| ring.iter().map(SerializablePoint::from).collect()).collect();
    }
}


#[derive(Deserialize, Clone)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}


#[derive(Deserialize, Clone)]
enum Filter {
    OneOf {
        key: String,
        valid_values: Vec<String>,
    },
    GreaterThan {
        key: String,
        cutoff: f64,
    },
    Not {
        filter: std::boxed::Box<Filter>,
    }
}

impl Filter {
    fn matches(self: &Filter, record: &Record) -> Result<bool, String> {
        match self {
            Filter::OneOf { key, valid_values } => {
                let value = record.get(&key).ok_or(format!("you can't filter on the field '{}' because it doesn't exist.", key))?;
                match value {
                    FieldValue::Numeric(Some(number)) => Ok(valid_values.contains(&f64::to_string(number))),
                    FieldValue::Numeric(None) => Ok(false),
                    FieldValue::Character(Some(characters)) => Ok(valid_values.contains(characters)),
                    FieldValue::Character(None) => Ok(false),
                    _ => return Err(String::from("you can only filter on numerical and character fields right now.")),
                }
            }
            Filter::GreaterThan { key, cutoff } => {
                let value = record.get(&key).ok_or(format!("you can't filter on the field '{}' because it doesn't exist.", key))?;
                match value {
                    FieldValue::Numeric(Some(number)) => Ok(number > cutoff),
                    FieldValue::Float(Some(number)) => Ok(&f64::from(*number) > cutoff),
                    FieldValue::Float(None) => Ok(false),
                    other => return Err(format!("GreaterThan filters must act on numeric or float32 fields, but '{}' is a {}.", key, other.field_type().to_string())),
                }
            }
            Filter::Not { filter } => {
                return match filter.matches(record) {
                    Ok(value) => Ok(!value),
                    Err(message) => Err(message),
                }
            }
        }
    }
}


#[derive(Deserialize, Clone)]
enum Transform {
    /// a linear scaling and translation from map to map
    Affine {
        /// the scale in the horizontal direction (mm per shapefile units)
        x_scale: f64,
        /// the x-coordinate of the shapefile origin (mm)
        x_shift: f64,
        /// the scale in the vertical direction (mm per shapefile units, should be negative)
        y_scale: f64,
        /// the y-coordinate of the shapefile origin (mm)
        y_shift: f64,
    },
    /// a cylindrical conformal mapping from spherical coordinates in degrees to a plane
    Mercator {
        /// the central meridian (°)
        central_meridian: f64,
        /// the one divided by map scale at the equator (mm/mm)
        equatorial_scale: f64,
    },
    /// a 3D rotation that gets applied before some other transformation
    Oblique {
        /// the latitude of the point that will appear at the top of the map (°)
        pole_latitude: f64,
        /// the longitude of the point that will appear at the top of the map (°)
        pole_longitude: f64,
        /// the transform to apply after rotating
        projection: std::boxed::Box<Transform>,
    },
}

impl Transform {
    fn between(input: &Box, output: &Box) -> Transform {
        let x_scale = (output.right - output.left)/(input.right - input.left);
        let x_shift = output.left - input.left*x_scale;
        let y_scale = (output.bottom - output.top)/(input.bottom - input.top);
        let y_shift = output.top - input.top*y_scale;
        return Transform::Affine { x_scale: x_scale, x_shift: x_shift, y_scale: y_scale, y_shift: y_shift };
    }

    fn apply(self: &Transform, input: &SerializablePoint) -> SerializablePoint {
        return match self {
            Transform::Affine { x_scale, x_shift, y_scale, y_shift } => {
                SerializablePoint {
                    x: input.x*x_scale + x_shift,
                    y: input.y*y_scale + y_shift,
                }
            },
            Transform::Mercator { central_meridian, equatorial_scale } => {
                SerializablePoint {
                    x: 1./equatorial_scale*EARTH_RADIUS*f64::to_radians(input.x - central_meridian),
                    y: -1./equatorial_scale*EARTH_RADIUS*f64::atanh(f64::sin(f64::to_radians(input.y))),
                }
            },
            Transform::Oblique { pole_latitude, pole_longitude, projection } => {
                let φ_pole = f64::to_radians(*pole_latitude);
                let λ_pole = f64::to_radians(*pole_longitude);
                let φ_point = f64::to_radians(input.y);
                let λ_point = f64::to_radians(input.x);
                let y_pole = -φ_pole.cos();
                let z_pole = φ_pole.sin();
                let x_point = φ_point.cos()*(λ_point - λ_pole).sin();
                let y_point = -φ_point.cos()*(λ_point - λ_pole).cos();
                let z_point = φ_point.sin();
                let φ_relative = (y_pole*y_point + z_pole*z_point).asin();
                let λ_relative = f64::atan2(x_point, y_pole*z_point - z_pole*y_point);
                Transform::apply(projection, &SerializablePoint {
                    x: f64::to_degrees(λ_relative),
                    y: f64::to_degrees(φ_relative),
                })
            },
        };
    }

    fn apply_to_box(self: &Transform, input: &Box) -> Result<Box, String> {
        return match self {
            Transform::Affine { x_scale, x_shift, y_scale, y_shift } => Ok(Box {
                left: input.left*x_scale + x_shift,
                right: input.right*x_scale + x_shift,
                bottom: input.bottom*y_scale + y_shift,
                top: input.top*y_scale + y_shift,
            }),
            Transform::Mercator { central_meridian, equatorial_scale } => Ok(Box {
                left: 1./equatorial_scale*EARTH_RADIUS*f64::to_radians(input.left - central_meridian),
                right: 1./equatorial_scale*EARTH_RADIUS*f64::to_radians(input.right - central_meridian),
                bottom: -1./equatorial_scale*EARTH_RADIUS*f64::atanh(f64::sin(f64::to_radians(input.bottom))),
                top: -1./equatorial_scale*EARTH_RADIUS*f64::atanh(f64::sin(f64::to_radians(input.top))),
            }),
            Transform::Oblique { .. } => Err(format!(
                "oblique map projections are not generally cylindrical and so I don't support projecting rectangles in them."
            )),
        };
    }

    fn apply_to_curve(self: &Transform, input: &Vec<SerializablePoint>, tolerance: f64) -> Vec<SerializablePoint> {
        // first, do the transform to all the points in the input
        let mut pending_points: Vec<(SerializablePoint, SerializablePoint)> = Vec::with_capacity(input.len());
        for raw_point in input {
            let transformed_point = self.apply(raw_point);
            pending_points.push((raw_point.clone(), transformed_point));
        }
        pending_points.reverse();
        // then inspect them and put them in the finished list one at a time
        let mut finished_points: Vec<(SerializablePoint, SerializablePoint)> = Vec::with_capacity(input.len());
        // the first one is a freebie
        match pending_points.pop() {
            Some(initial_point) => finished_points.push(initial_point),
            None => {},
        }

        // go thru until there are no more points pending
        while pending_points.len() > 0 {
            let (last_raw, last_transformed) = finished_points.get(finished_points.len() - 1).unwrap();
            let (next_raw, next_transformed) = pending_points.pop().unwrap();
            // look at the point between the last finished point and the next pending point
            let tween_raw = SerializablePoint { x: (last_raw.x + next_raw.x)/2., y: (last_raw.y + next_raw.y)/2. };
            let tween_transformed = self.apply(&tween_raw);
            // see how far it is from the line segment between its neibors
            let distance = if self.is_affine() {0.} else {
                line_point_distance(last_transformed, &next_transformed, &tween_transformed)
            };
            // if it's pretty close, discard the midpoint and save the pending point
            if distance < tolerance {
                finished_points.push((next_raw, next_transformed));
            }
            // if it's significantly off, put it back on the queue along with the midpoint
            else {
                pending_points.push((next_raw, next_transformed));
                pending_points.push((tween_raw, tween_transformed));
            }
        }

        // finally, remove all the raw points so only transformed points are left
        let mut output = Vec::with_capacity(finished_points.len());
        for (_, transformed_point) in finished_points {
            output.push(transformed_point);
        }
        return output;
    }

    fn is_affine(self: &Transform) -> bool {
        return match self {
            Transform::Affine {..} => true,
            Transform::Mercator {..} => false,
            Transform::Oblique {..} => false,
        }
    }
}


#[cfg(test)]
mod test;
