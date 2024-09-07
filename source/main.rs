use std::fs;
use serde::{Deserialize};
use shapefile::dbase::FieldValue;
use shapefile::Shape;

fn main() -> () {
    let yaml = fs::read_to_string(
        "configurations/congresentatives.yml").unwrap();
    let configuration: Configuration = serde_yaml::from_str(&yaml).unwrap();
    let mut svg_code = format!(
        "\
<svg viewBox=\"0 700000 400000 300000\" width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">
  <title>{}</title>
  <style>
{}
  </style>
\
        ",
        configuration.bounding_box.right - configuration.bounding_box.left,
        configuration.bounding_box.bottom - configuration.bounding_box.top,
        configuration.title, configuration.style,
    );

    svg_code.push_str(&format!("  <g class=\"{}\">\n", configuration.content.class));
    let mut reader = shapefile::Reader::from_path(format!("data/{}.shp", configuration.content.filename)).unwrap();
    for shape_record in reader.iter_shapes_and_records() {
        let shape_record = shape_record.unwrap();
        let (shape, record) = shape_record;
        let identifier: Option<String> = match record.get("DIST_NUM") {
            Some(value) => match value {
                FieldValue::Character(character) => character.clone(),
                FieldValue::Numeric(number) => match number {
                    Some(number) => Some(format!("{}-{}", configuration.content.class, number)),
                    None => None,
                },
                _ => None,
            },
            None => None,
        };
        match shape {
            Shape::Polygon(polygon) => {
                let mut path_string = String::new();
                for ring in polygon.rings() {
                    for (i, point) in ring.points().iter().enumerate() {
                        path_string.push_str(&format!("{}{:.3},{:.3} ", if i == 0 {"M"} else {"L"}, point.x, point.y));
                    }
                }
                svg_code.push_str(&format!(
                    "    <path {}d=\"{}\" />\n",
                    match identifier { Some(string) => format!("class=\"{}\" ", string), None => String::from("")},
                    path_string));
            }
            _ => {
                panic!("we only do polygons right now.");
            }
        }
    }
    svg_code.push_str("  </g>\n");
    svg_code.push_str("</svg>\n");

    let image_filename = "maps/congresentatives.svg";
    fs::create_dir_all("maps/").unwrap();
    fs::write(image_filename, svg_code).unwrap();
}

#[derive(Deserialize)]
struct Configuration {
    title: String,
    style: String,
    bounding_box: Box,
    content: Layer,
}

#[derive(Deserialize)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

#[derive(Deserialize)]
struct Layer {
    class: String,
    filename: String,
}
