#![allow(non_snake_case)]

use core::f64;
use regex::Regex;
use std::{env, fs, iter, marker};
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
    for content in configuration.contents {
        let content = load_content(content, &configuration.region)?;
        let content = transform_content(content, &map_bounding_box, &configuration.region)?;
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
        Content::Group { contents: sub_contents, bounding_box, region: sub_region, frame, class } => {
            let region: &Option<Box> = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            let mut loaded_sub_contents = Vec::with_capacity(sub_contents.len());
            for sub_content in sub_contents {
                loaded_sub_contents.push(load_content(sub_content, region)?);
            }
            // add in some rectangles if desired
            match frame {
                Some(true) => {
                    let region = match region {
                        Some(region) => region,
                        None => return Err(String::from("if a Group has frame: true, it must also set a region.")),
                    };
                    loaded_sub_contents.insert(
                        0, Content::Rectangle {
                            coordinates: region.clone(),
                            class: Some(String::from("background")),
                        },
                    );
                    loaded_sub_contents.push(
                        Content::Rectangle {
                            coordinates: region.clone(),
                            class: Some(String::from("frame")),
                        },
                    );
                }
                _ => {}
            }
            return Ok(Content::Group {
                contents: loaded_sub_contents,
                region: sub_region,
                bounding_box, class,
                frame: Some(false),
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
                frame: None,
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
                frame: None,
            });
        }

        // any other form of content can just continue on as it is
        other => Ok(other),
    }
}


/// apply all necessary coordinate transforms to the points in this box.
/// all geographic data should come out in SVG coordinates rather than in shapefile coordinates.
fn transform_content(content: Content, outer_bounding_box: &Box, outer_region: &Option<Box>) -> Result<Content, String> {
    match content {
        Content::Group { contents: sub_contents, bounding_box: sub_bounding_box, region: sub_region, frame, class } => {
            let bounding_box = match &sub_bounding_box {
                Some(sub_bounding_box) => sub_bounding_box,
                None => outer_bounding_box,
            };
            let region = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            let mut transformed_contents = Vec::with_capacity(sub_contents.len());
            for sub_content in sub_contents {
                transformed_contents.push(transform_content(sub_content, bounding_box, region)?);
            }
            return Ok(Content::Group {
                contents: transformed_contents,
                bounding_box: sub_bounding_box,
                region: sub_region,
                frame: frame,
                class: class,
            });
        },
        Content::Line { start, end, class } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);
            return Ok(Content::Line {
                start: Transform::apply(&transform, start),
                end: Transform::apply(&transform, end),
                class: class,
            });
        },
        Content::Rectangle { coordinates, class } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);
            return Ok(Content::Rectangle {
                coordinates: Transform::apply_to_box(&transform, coordinates),
                class: class,
            });
        },
        Content::Path { parts, closed, self_clip, class } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);
            let mut transformed_parts = Vec::with_capacity(parts.len());
            for part in parts {
                let mut transformed_part = Vec::with_capacity(part.len());
                for point in part {
                    transformed_part.push(Transform::apply(&transform, point));
                }
                transformed_parts.push(transformed_part);
            }
            return Ok(Content::Path {
                parts: transformed_parts,
                closed: closed,
                self_clip: self_clip,
                class: class,
            });
        },
        Content::Marker { detail, location, size, class } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);
            return Ok(Content::Marker {
                detail: detail,
                location: Transform::apply(&transform, location),
                size: size,
                class: class,
            });
        },
        Content::Label { text, location: coordinates, class } => {
            let region = outer_region.as_ref().ok_or(String::from(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            let transform = Transform::between(region, outer_bounding_box);
            return Ok(Content::Label {
                text: text,
                location: Transform::apply(&transform, coordinates),
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
        Content::Group{ contents: sub_contents, bounding_box: sub_bounding_box, region: sub_region, .. } => {
            // this group may override the outer bounding box and region
            let bounding_box = match &sub_bounding_box {
                Some(sub_bounding_box) => sub_bounding_box,
                None => outer_bounding_box,
            };
            let region = match &sub_region {
                Some(..) => &sub_region,
                None => outer_region,
            };
            let group_index = *element_count;
            // write all the stuff
            let mut string = String::new();
            string.push_str(&format!("<clipPath id=\"clip_path_{}\">\n", group_index));
            string.push_str(&format!(
                "  <rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\"/>\n",
                bounding_box.left, bounding_box.top,
                bounding_box.right - bounding_box.left,
                bounding_box.bottom - bounding_box.top,
            ));
            string.push_str(&format!("</clipPath>\n"));
            string.push_str(&format!("<g clip-path=\"url(#clip_path_{})\">\n", group_index));
            for sub_content in sub_contents {
                string.push_str(&transcribe_content_as_svg(sub_content, bounding_box, region, element_count)?);
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
                    "translate({}, {}) scale({})",
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


#[derive(Deserialize)]
struct Configuration {
    title: String,
    description: String,
    style: String,
    bounding_box: Option<Box>,
    region: Option<Box>,
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
        /// whether to duplicate each shape in this thing
        double: Option<bool>,
        /// whether to make this shape's strokes be confined within its shape
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
        /// the box to which to fit this group's contents
        bounding_box: Option<Box>,
        /// the geographical region to include. shapes wholly outside this region will be discarded,
        /// and the contents will be scaled to fit this region to the bounding box.
        region: Option<Box>,
        /// whether to add a rect for a background and frame
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


#[derive(Deserialize, Clone)]
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

    fn apply(self: &Transform, input: SerializablePoint) -> SerializablePoint {
        return SerializablePoint {
            x: input.x*self.x_scale + self.x_shift,
            y: input.y*self.y_scale + self.y_shift,
        };
    }

    fn apply_to_box(self: &Transform, input: Box) -> Box {
        return Box {
            left: input.left*self.x_scale + self.x_shift,
            right: input.right*self.x_scale + self.x_shift,
            top: input.top*self.y_scale + self.y_shift,
            bottom: input.bottom*self.y_scale + self.y_shift,
        }
    }
}
