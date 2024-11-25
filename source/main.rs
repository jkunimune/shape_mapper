#![allow(non_snake_case)]

use anyhow::{anyhow, Result};
use core::f64;
use std::f64::consts::PI;
use regex::Regex;
use std::{boxed, env, fs};
use serde::Deserialize;
use shapefile::dbase::{FieldValue, Record};
use shapefile::record::EsriShape;
use shapefile::Shape;
use titlecase::titlecase;



const EARTH_RADIUS: f64 = 6.371e9; // mm (TODO: infer this from the .prj file?)
const CURVE_PRECISION: f64 = 0.1; // mm



fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() > 2 {
        return Err(anyhow!("you must pass only one argument, representing the filename of the configuration file without the 'yml'"));
    }
    let filename = args.get(1).ok_or(anyhow!("Please pass the filename of the configuration file minus the 'yml' as a command line argument."))?;

    let file = fs::read_to_string(format!("configurations/{}.yml", filename));
    let yaml = match file {
        Ok(yaml) => yaml,
        Err(message) => return Err(anyhow!("I could not read 'configurations/{}.yml' because {}", filename, message)),
    };
    let configuration: Configuration = serde_yaml::from_str(&yaml)?;

    println!("generating a map of '{}' based on `configurations/{}.yml`.", configuration.title, filename);

    let map_bounding_box = configuration.bounding_box.ok_or(anyhow!("the top level must have a bounding box"))?;
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
        let content = transform_content(content, &top_level_transform, &Some(map_bounding_box.clone()))?;
        let content = transcribe_content_as_svg(content, &map_bounding_box, &configuration.region, element_index)?;
        let content = prepend_to_each_line(&content, "  ");
        svg_code.push_str(&content);
    }

    svg_code.push_str("</svg>\n");

    println!("saving `maps/{}.svg.`", filename);

    fs::create_dir_all("maps/")?;
    fs::write(format!("maps/{}.svg", filename), svg_code)?;

    println!("done!");
    return Ok(());
}


/// return a copy of this content that is the same except that any Layers and Graticules
/// are resolved into groups of Paths
fn load_content(content: Content, outer_region: &Option<Box>) -> Result<Content> {
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

        // for a ClipPath, similarly just load its child
        Content::ClipPath { content, id } => {
            return Ok(Content::ClipPath {
                id, content: boxed::Box::new(load_content(*content, outer_region)?),
            });
        }

        // resolve a Layer by loading geographic data from disc and making a bunch of Paths or Markers
        Content::Layer { filename, class, class_column, label_column, case, abbr, marker_name, size, double, self_clip, decimation, filters } => {
            let region = outer_region.as_ref().ok_or(anyhow!(
                "every layer must have a region defined somewhere in its hierarchy."))?;
            // check for incompatible options
            if label_column.is_some() && marker_name.is_some() {
                return Err(anyhow!("you may not use both `label_column` and `marker_name` in a single Layer."))
            }
            if label_column.is_none() && case.is_some() {
                return Err(anyhow!("you may not use the `case` option without the `label_column` option."))
            }
            if label_column.is_none() && abbr.is_some() {
                return Err(anyhow!("you may not use the `abbr` option without the `label_column` option."))
            }
            if marker_name.is_none() && size.is_some() {
                return Err(anyhow!("you may not use the `size` option without the `marker_name` option."));
            }
            if marker_name.is_some() && self_clip.is_some() {
                return Err(anyhow!("the `self_clip` option is incompatible with the `marker_name` option."));
            }

            let marker_data = match &marker_name {
                // if marker was unspecified, don't load anything
                None => None,
                // if marker was specified, load its content from disc and unwrap marker_size
                Some(marker_filename) => {
                    // make sure we have a marker size
                    let marker_size = match size {
                        Some(number) => number,
                        None => {
                            return Err(anyhow!("the `{}.shp` layer has a marker, but no marker size is given.", filename));
                        }
                    };
                    // check for any incompatible options
                    match self_clip {
                        Some(true) => {
                            return Err(anyhow!("the `self_clip` option is incompatible with the `marker` option."));
                        }
                        _ => {}
                    }
                    Some((load_SVG(&marker_filename)?, marker_size))
                }
            };

            let mut contents = Vec::new();
            let mut reader = shapefile::Reader::from_path(
                format!("data/{}.shp", filename)).or(Err(anyhow!("could not find `data/{}.dbf`", &filename)))?;

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
                            _ => return Err(anyhow!("I don't know how to print this field type.")),
                        },
                        None => return Err(anyhow!("there doesn't seem to be a '{}' collum in '{}.shp'.", class_column, &filename)),
                    }
                    None => None,
                };

                let location = center_of(&shape)?;

                // convert the shape to a Path or a Marker or a Label
                let shape = match &marker_data {
                    None => match &label_column {
                        None => {
                            // if no marker or label was specified, try to make a Content::Path
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
                                Shape::PolylineM(polyline) => {
                                    (SerializablePoint::deep_convert_M(polyline.parts()), false)
                                }
                                Shape::Point(_) => {
                                    return Err(anyhow!("data/{}.shp is a POINT shapefile.  POINT layers must always have a `marker`.", filename))
                                }
                                _ => {
                                    return Err(anyhow!("we don't support {} shapefiles right now.", shape.shapetype().to_string()));
                                }
                            };

                            let parts = match decimation {
                                Some(tolerance) => parts.iter().map(|part| decimate(part, tolerance)).collect(),
                                None => parts,
                            };

                            // pull it all together as a Content::Path
                            Some(Content::Path {
                                parts, closed, self_clip,
                                class: shape_class,
                            })
                        }
                        Some(label_column) => {
                            // if a label was specified
                            // decide what the label should say
                            let text = match record.get(label_column) {
                                Some(FieldValue::Character(characters)) => match characters {
                                    Some(characters) => characters,
                                    None => continue,
                                },
                                Some(value) => {
                                    return Err(anyhow!("you can only label by string columns, but '{}' is a {} column.", label_column, value.field_type().to_string()));
                                }
                                None => {
                                    return Err(anyhow!("you can't label by the column '{}' because it doesn't seem to exist.", label_column));
                                }
                            };
                            // modify the case if desired
                            let text = match case {
                                Some(Case::Upper) => text.to_uppercase(),
                                Some(Case::Lower) => text.to_lowercase(),
                                Some(Case::Sentence) => text[..1].to_uppercase() + &text[1..].to_lowercase(),
                                Some(Case::Title) => titlecase(text),
                                None => text.to_owned(),
                            };
                            let text = match &abbr {
                                Some(replacements) => {
                                    let mut new_text = text;
                                    for Replacement {from, to} in replacements.iter() {
                                        new_text = new_text.replace(from, &to);
                                    }
                                    new_text
                                }
                                None => text,
                            };
                            // add necessary escape sequences (make sure you do this after setting the case)
                            let text = sanitize_XML(&text);
                            Some(Content::Label {
                                text, location,
                                class: shape_class,
                            })
                        }
                    }
                    Some((marker_detail, marker_size)) => {
                        // if marker was specified, make a Content::Marker
                        let marker_location = center_of(&shape)?;
                        Some(Content::Marker {
                            detail: Some(marker_detail.clone()),
                            filename: None,
                            location: marker_location,
                            size: *marker_size,
                            bearing: None,
                            class: shape_class,
                        })
                    }
                };

                // append it to the list, twice if so desired
                match shape {
                    Some(shape) => match double {
                        Some(true) => {
                            contents.push(Content::Group {
                                bounding_box: None, region: None, transform: None,
                                clip: Some(false), frame: Some(false),
                                class: shape.get_class().clone(),
                                contents: vec![
                                    shape.clone().set_class(Some("bottom".to_string())),
                                    shape.clone().set_class(Some("top".to_string())),
                                ],
                            });
                        }
                        _ => {
                            contents.push(shape);
                        }
                    }
                    None => {}
                }
            }

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
                None => return Err(anyhow!(
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

        // for markers, if a filename is given, load the file from disc
        Content::Marker { detail, filename, location, size, bearing, class } => {
            let detail = match detail {
                Some(detail) => match filename {
                    Some(_) => return Err(anyhow!("you shouldn't specify both a detail and a filename for a Marker.")),
                    None => detail,
                },
                None => match filename {
                    Some(filename) => load_SVG(&filename)?,
                    None => return Err(anyhow!("you should specify one of a detail or a filename for a Marker."))
                },
            };
            return Ok(Content::Marker {
                detail: Some(detail), filename: None,
                location, size, bearing, class
            })
        }

        // any other form of content can just continue on as it is
        other => Ok(other),
    }
}


/// apply all necessary coordinate transforms to the points in this box.
/// all geographic data should come out in SVG coordinates rather than in shapefile coordinates.
fn transform_content(content: Content, outer_transform: &Option<Transform>, outer_bounding_box: &Option<Box>) -> Result<Content> {
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
            let innermost_bounding_box = match bounding_box {
                Some(..) => &bounding_box,
                None => outer_bounding_box,
            };
            let mut transformed_contents = Vec::with_capacity(sub_contents.len());
            for sub_content in sub_contents {
                transformed_contents.push(transform_content(sub_content, transform, innermost_bounding_box)?);
            }
            return Ok(Content::Group {
                contents: transformed_contents,
                bounding_box, region,
                clip, frame, class,
                transform: None,
            });
        },
        Content::ClipPath { content, id } => {
            return Ok(Content::ClipPath {
                content: boxed::Box::new(transform_content(
                    *content, outer_transform, outer_bounding_box)?
                ),
                id });
        },
        Content::Line { start, end, class } => {
            let transform = outer_transform.as_ref().ok_or(anyhow!(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Line {
                start: Transform::apply(&transform, &start),
                end: Transform::apply(&transform, &end),
                class: class,
            });
        },
        Content::Rectangle { coordinates, class } => {
            let transform = outer_transform.as_ref().ok_or(anyhow!(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Rectangle {
                coordinates: Transform::apply_to_box(&transform, &coordinates)?,
                class: class,
            });
        },
        Content::Path { parts, closed, self_clip, class } => {
            let transform = outer_transform.as_ref().ok_or(anyhow!(
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
        Content::Marker { detail, filename, location, size, bearing, class } => {
            let transform = outer_transform.as_ref().ok_or(anyhow!(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            return Ok(Content::Marker {
                detail, filename, size, class,
                location: Transform::apply(&transform, &location),
                bearing: match bearing {
                    Some(bearing) => {
                        let north_vector = Transform::north_direction_at(&transform, &location);
                        Some(bearing + f64::atan2(north_vector.x, -north_vector.y).to_degrees())
                    },
                    None => None,
                },
            });
        },
        Content::Label { text, location: coordinates, class } => {
            let transform = outer_transform.as_ref().ok_or(anyhow!(
                "every layer must have a region defined somewhere in its hierarchy."
            ))?;
            let location = Transform::apply(&transform, &coordinates);
            let location = match outer_bounding_box {
                Some(outer_bounding_box) => coerce_in_box(&location, outer_bounding_box, 2.),
                None => location,
            };
            return Ok(Content::Label { text, location, class, });
        },
        Content::Layer { .. } => {
            return Err(anyhow!("Layers should have been purged by now"));
        },
        Content::Graticule { .. } => {
            return Err(anyhow!("Graticules should have been purged by now"));
        },
    }
}


fn transcribe_content_as_svg(content: Content, outer_bounding_box: &Box, outer_region: &Option<Box>, element_count: &mut u32) -> Result<String> {
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

        // for a clipPath, transcribe its child wrapped in a <clipPath> tag
        Content::ClipPath { content, id } => {
            let content = transcribe_content_as_svg(*content, outer_bounding_box, outer_region, element_count)?;
            // remove all <clipPath>s inside the content because we don't currently support nested clip paths
            let internal_clip_pattern = Regex::new("(?s)<clipPath[^>]*>.*</clipPath>\n?").unwrap();
            let content = internal_clip_pattern.replace_all(&content, "");
            // remove all <g> tags because <clipPath>s can't have hierarchy
            let hierarchical_tag_pattern = Regex::new("(?m)^.*[^/]>\n").unwrap();
            let content = hierarchical_tag_pattern.replace_all(&content, "");
            // wrap it in a new <clipPath>
            format!("<clipPath id=\"{}\">\n{}</clipPath>\n", id, content)
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

        Content::Marker { detail, location, size, bearing, .. } =>
            match detail {
                Some(detail) =>
                    insert_attribute(
                    &detail, "transform",
                    &format!(
                        "translate({:.2}, {:.2}) scale({:.4}) rotate({:.1})",
                        location.x, location.y, f64::sqrt(size), bearing.unwrap_or(0.),
                    ),
                )?,
                None => return Err(anyhow!("this marker should have had its detail filled in by now.")),
            },

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
            return Err(anyhow!("the graticules should have all been purged by now."));    
        }
        Content::Layer{ .. } => {
            return Err(anyhow!("the Layers should have all been purged by now."));
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


fn load_SVG(marker_filename: &String) -> Result<String> {
    let marker_string = fs::read_to_string(format!("markers/{}.svg", marker_filename)).or(Err(anyhow!("couldn't read `markers/{}.svg`", marker_filename)))?;
    // extract the content from between the <svg> and </svg>
    let svg_captures = Regex::new(r"(?s)<svg[^>]*>\n(.*\n)\s*</svg>").unwrap().captures(&marker_string);
    let marker_string = match svg_captures {
        Some(svg_captures) => svg_captures.get(1).unwrap().as_str(),
        None => return Err(anyhow!("markers/{}.svg was malformed somehow.", marker_filename)),
    };
    // extract the top-level indentation so that you can remove it
    let indentation_captures = Regex::new(r"^([ \t]*)<").unwrap().captures(&marker_string);
    let indentation = match indentation_captures {
        Some(indentation_captures) => indentation_captures.get(1).unwrap().as_str(),
        None => return Err(anyhow!("markers/{}.svg didn't seem to start with a tag.", marker_filename)),
    };
    // remove that indentation and return
    let indentation_pattern = Regex::new(&format!("(?m)^{}", indentation)).unwrap();
    let marker_string = indentation_pattern.replace_all(marker_string, "");
    return Ok(String::from(marker_string));
}


fn center_of(shape: &Shape) -> Result<SerializablePoint> {
    return match bounds_of(shape) {
        Ok([x_range, y_range]) => {
            Ok(SerializablePoint { x: (x_range[0] + x_range[1])/2., y: (y_range[0] + y_range[1])/2. })
        }
        Err(_) => {
            Err(anyhow!("I cannot calculate the center of this shape because it has no geometry."))
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


fn coerce_in_box(point: &SerializablePoint, boks: &Box, buffer: f64) -> SerializablePoint {
    if boks.bottom > boks.top {
        return coerce_in_box(point, &Box {
            left: boks.left, right: boks.right, bottom: boks.top, top: boks.bottom,
        }, buffer);
    }
    return SerializablePoint {
        x: f64::max(boks.left + buffer, f64::min(boks.right - buffer, point.x)),
        y: f64::max(boks.bottom + buffer, f64::min(boks.top - buffer, point.y)),
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
fn insert_attribute(element: &str, key: &str, value: &str) -> Result<String> {
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
        return Err(anyhow!("I couldn't find any top-level tags in the string '{}'", element));
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
    let string = string.to_lowercase();
    let string = Regex::new(r"[{},.:;]").unwrap().replace_all(&string, "_").into_owned();
    let string = Regex::new(r"^([0-9])").unwrap().replace(&string, "_$1").into_owned();
    return string;
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


/// use the Ramer–Douglas–Peucker algorithm to downsample this curve
fn decimate(path: &[SerializablePoint], tolerance: f64) -> Vec<SerializablePoint> {
    let mut max_distance = 0.;
    let mut argmax_distance = 0;
    for i in 0..path.len() {
        let distance = line_point_distance(&path[0], &path[path.len() - 1], &path[i]);
        if distance > max_distance {
            max_distance = distance;
            argmax_distance = i;
        }
    }
    if max_distance > tolerance {
        let head = decimate(&path[..argmax_distance + 1], tolerance);
        let tail = decimate(&path[argmax_distance..], tolerance);
        return [head, tail].concat();
    }
    else {
        return vec![path[0].clone(), path[path.len() - 1].clone()];
    }
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
        case: Option<Case>,
        /// a set of replacements to make in each label text, if any
        abbr: Option<Vec<Replacement>>,
        /// the name of the SVG (without the 'markers/' or '.svg') to put at the center of this thing
        marker_name: Option<String>,
        /// the desired area of the marker in square millimeters
        size: Option<f64>,
        /// whether to duplicate each shape in this thing (defaults to false)
        double: Option<bool>,
        /// whether to make this shape's strokes be confined within its shape (defaults to false)
        self_clip: Option<bool>,
        /// the tolerance to use for the decimation algorithm you apply to the curves (in data units), if any
        decimation: Option<f64>,
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
        detail: Option<String>,
        /// the name of the SVG (without the 'markers/' or '.svg') to put at the center of this thing
        filename: Option<String>,
        /// the coordinates in geographical space to which the marker detail's origin will be shifted
        location: SerializablePoint,
        /// a multiplier that will be applied to the shape's area
        size: f64,
        /// a rotation about its center to apply, in degrees, clockwise, if any
        bearing: Option<f64>,
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
    /// a <clipPath/> tag that wraps some other element
    ClipPath {
        /// the content being wraped
        content: boxed::Box<Content>,
        /// the id to assign to the clipPath
        id: String,
    }
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
            Content::ClipPath { .. } => &None,
        };
    }

    fn set_class(self: Content, class: Option<String>) -> Content {
        return match self {
            Content::Path { parts, closed, self_clip, class: _ } => Content::Path {
                class, parts, closed, self_clip,
            },
            Content::Label { text, location, class: _ } => Content::Label {
                class, text, location,
            },
            Content::Marker { detail, filename, location, size, bearing, class: _ } => Content::Marker {
                class, detail, filename, location, size, bearing,
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
    Title,
}


#[derive(Deserialize, Clone)]
struct Replacement {
    from: String,
    to: String,
}


/// a pair of Cartesian coordinates
#[derive(Deserialize, Clone, PartialEq, Debug)]
struct SerializablePoint {
    x: f64,
    y: f64,
}

impl SerializablePoint {
    fn from(point: &shapefile::Point) -> SerializablePoint {
        return SerializablePoint { x: point.x, y: point.y };
    }
    fn from_M(point: &shapefile::PointM) -> SerializablePoint {
        return SerializablePoint { x: point.x, y: point.y };
    }
    fn deep_convert(points: &Vec<Vec<shapefile::Point>>) -> Vec<Vec<SerializablePoint>> {
        return points.iter().map(|ring| ring.iter().map(SerializablePoint::from).collect()).collect();
    }
    fn deep_convert_M(points: &Vec<Vec<shapefile::PointM>>) -> Vec<Vec<SerializablePoint>> {
        return points.iter().map(|ring| ring.iter().map(SerializablePoint::from_M).collect()).collect();
    }
}


/// a set of four coordinates that defines a Cartesian rectangle
#[derive(Deserialize, Clone)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}


/// a gradient of a Point -> Point function
#[derive(Debug)]
struct Jacobian {
    dx_dx: f64, dx_dy: f64,
    dy_dx: f64, dy_dy: f64,
}

impl Jacobian {
    /// do matrix multiplication to chain two Jacobians together
    fn times(self: Jacobian, inner: &Jacobian) -> Jacobian {
        return Jacobian {
            dx_dx: self.dx_dx*inner.dx_dx + self.dx_dy*inner.dy_dx,
            dx_dy: self.dx_dx*inner.dx_dy + self.dx_dy*inner.dy_dy,
            dy_dx: self.dy_dx*inner.dx_dx + self.dy_dy*inner.dy_dx,
            dy_dy: self.dy_dx*inner.dx_dy + self.dy_dy*inner.dy_dy,
        }
    }
}


#[derive(Deserialize, Clone)]
enum Filter {
    /// check that a record value is one of the values in the given list
    OneOf {
        key: String,
        valid_values: Vec<String>,
    },
    /// check that a record value is greater than a threshold
    GreaterThan {
        key: String,
        cutoff: f64,
    },
    /// the inverse of some other given Filter
    Not {
        filter: boxed::Box<Filter>,
    },
    /// the 
    Either {
        filters: Vec<Filter>,
    },
}

impl Filter {
    fn matches(self: &Filter, record: &Record) -> Result<bool> {
        match self {
            Filter::OneOf { key, valid_values } => {
                let value = record.get(&key).ok_or(anyhow!("you can't filter on the field '{}' because it doesn't exist.", key))?;
                match value {
                    FieldValue::Numeric(Some(number)) => Ok(valid_values.contains(&f64::to_string(number))),
                    FieldValue::Numeric(None) => Ok(false),
                    FieldValue::Character(Some(characters)) => Ok(valid_values.contains(characters)),
                    FieldValue::Character(None) => Ok(false),
                    _ => return Err(anyhow!("you can only filter on numerical and character fields right now.")),
                }
            }
            Filter::GreaterThan { key, cutoff } => {
                let value = record.get(&key).ok_or(anyhow!("you can't filter on the field '{}' because it doesn't exist.", key))?;
                match value {
                    FieldValue::Numeric(Some(number)) => Ok(number > cutoff),
                    FieldValue::Float(Some(number)) => Ok(&f64::from(*number) > cutoff),
                    FieldValue::Float(None) => Ok(false),
                    other => return Err(anyhow!("GreaterThan filters must act on numeric or float32 fields, but '{}' is a {}.", key, other.field_type().to_string())),
                }
            }
            Filter::Not { filter } => {
                return match filter.matches(record) {
                    Ok(value) => Ok(!value),
                    Err(message) => Err(message),
                }
            }
            Filter::Either { filters } => {
                for filter in filters {
                    if filter.matches(record)? {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
        }
    }
}


#[derive(Deserialize, Clone)]
enum Transform {
    /// a linear translation, scaling, and rotation from map to map
    Affine {
        /// the scale in the x-direction (m per shapefile units)
        longitudinal_scale: f64,
        /// the x-coordinate of the point to put at the map origin (shapefile units)
        false_easting: f64,
        /// the scale in the y-direction (m per shapefile units, should be negative)
        latitudinal_scale: f64,
        /// the y-coordinate of the point to put at the map origin (shapefile units)
        false_northing: f64,
        /// the amount to rotate the data widdershins about the map origin (degrees)
        rotation: f64,
    },
    /// a cylindrical conformal mapping from spherical coordinates in degrees to a plane
    Mercator {
        /// the central meridian (°)
        central_meridian: f64,
        /// the scale of the map at the equator (mm per real-life mm)
        scale: f64,
    },
    /// a 3D rotation that gets applied before some other transformation
    Oblique {
        /// the latitude of the point that will appear at the top of the map (°)
        pole_latitude: f64,
        /// the longitude of the point that will appear at the top of the map (°)
        pole_longitude: f64,
        /// the transform to apply after rotating
        projection: boxed::Box<Transform>,
    },
}

impl Transform {
    fn between(input: &Box, output: &Box) -> Transform {
        let longitudinal_scale = (output.right - output.left)/(input.right - input.left)/1000.;
        let false_easting = output.left/(1000.*longitudinal_scale) - input.left;
        let latitudinal_scale = (output.bottom - output.top)/(input.bottom - input.top)/1000.;
        let false_northing = output.top/(1000.*latitudinal_scale) - input.top;
        let rotation = 0.;
        return Transform::Affine {
            longitudinal_scale, false_easting,
            latitudinal_scale, false_northing,
            rotation };
    }

    fn apply(self: &Transform, input: &SerializablePoint) -> SerializablePoint {
        return match self {
            Transform::Affine { longitudinal_scale, false_easting, latitudinal_scale, false_northing, rotation } => {
                let x = (input.x + false_easting)*1000.*longitudinal_scale;
                let y = (input.y + false_northing)*1000.*latitudinal_scale;
                if *rotation == 0. {
                    SerializablePoint {x, y}
                }
                else {
                    let (sinθ, cosθ) = rotation.to_radians().sin_cos();
                    SerializablePoint {
                        x: cosθ*x + sinθ*y,
                        y: -sinθ*x + cosθ*y,
                    }
                }
            },
            Transform::Mercator { central_meridian, scale } => {
                SerializablePoint {
                    x: scale*EARTH_RADIUS*f64::to_radians(input.x - central_meridian),
                    y: -scale*EARTH_RADIUS*f64::atanh(f64::sin(f64::to_radians(input.y))),
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

    fn jacobian(self: &Transform, location: &SerializablePoint) -> Jacobian {
        return match self {
            Transform::Affine { longitudinal_scale, false_easting: _, latitudinal_scale, false_northing: _, rotation } => Jacobian {
                dx_dx: 1000.*longitudinal_scale*rotation.to_radians().cos(),
                dx_dy: 1000.*latitudinal_scale*rotation.to_radians().sin(),
                dy_dx: -1000.*longitudinal_scale*rotation.to_radians().sin(),
                dy_dy: 1000.*latitudinal_scale*rotation.to_radians().cos(),
            },
            Transform::Mercator { scale, central_meridian: _ } => Jacobian {
                dx_dx: scale*EARTH_RADIUS*PI/180., dx_dy: 0.,
                dy_dx: 0., dy_dy: -scale*EARTH_RADIUS*PI/180./f64::cos(location.y.to_radians()),
            },
            Transform::Oblique { pole_latitude, pole_longitude, projection } => {
                let φ_pole = f64::to_radians(*pole_latitude);
                let λ_pole = f64::to_radians(*pole_longitude);
                let φ_point = f64::to_radians(location.y);
                let λ_point = f64::to_radians(location.x);
                let y_pole = -φ_pole.cos();
                let z_pole = φ_pole.sin();
                let ρ_point = φ_point.cos();
                let x_point = ρ_point*(λ_point - λ_pole).sin();
                let y_point = -ρ_point*(λ_point - λ_pole).cos();
                let z_point = φ_point.sin();
                let ζ_relative = y_pole*y_point + z_pole*z_point;
                let ρ_relative = f64::sqrt(1. - ζ_relative.powi(2));
                let relative_location = SerializablePoint {
                    x: f64::to_degrees(f64::atan2(x_point, y_pole*z_point - z_pole*y_point)),
                    y: f64::to_degrees(ζ_relative.asin()),
                };
                return projection.jacobian(&relative_location).times(&Jacobian {
                    dx_dx: (z_pole*ρ_point.powi(2) - y_pole*z_point*y_point)/ρ_relative.powi(2),
                    dx_dy: -y_pole*(λ_point - λ_pole).sin()/ρ_relative.powi(2),
                    dy_dx: y_pole*x_point/ρ_relative,
                    dy_dy: (y_pole*z_point*(λ_point - λ_pole).cos() + z_pole*ρ_point)/ρ_relative,
                });
            },
        }
    }

    fn apply_to_box(self: &Transform, input: &Box) -> Result<Box> {
        return match self {
            Transform::Affine { longitudinal_scale, false_easting, latitudinal_scale, false_northing, rotation } => 
                if *rotation == 0. {
                    Ok(Box {
                        left: (input.left + false_easting)*1000.*longitudinal_scale,
                        right: (input.right + false_easting)*1000.*longitudinal_scale,
                        bottom: (input.bottom + false_northing)*1000.*latitudinal_scale,
                        top: (input.top + false_northing)*1000.*latitudinal_scale,
                    })
                }
                else {
                    Err(anyhow!("rotation is not supported with rectangles"))
                },
            Transform::Mercator { central_meridian, scale } =>
                Ok(Box {
                    left: scale*EARTH_RADIUS*f64::to_radians(input.left - central_meridian),
                    right: scale*EARTH_RADIUS*f64::to_radians(input.right - central_meridian),
                    bottom: -scale*EARTH_RADIUS*f64::atanh(f64::sin(f64::to_radians(input.bottom))),
                    top: -scale*EARTH_RADIUS*f64::atanh(f64::sin(f64::to_radians(input.top))),
                }),
            Transform::Oblique { .. } =>
                Err(anyhow!(
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

    /// a vector that points northward at this location
    fn north_direction_at(self: &Transform, location: &SerializablePoint) -> SerializablePoint {
        let jacobian = self.jacobian(location);
        return SerializablePoint { x: jacobian.dx_dy, y: jacobian.dy_dy };
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
