use std::fs;
use serde::{Deserialize};

fn main() -> () {
    let yaml = fs::read_to_string(
        "configurations/congresentatives.yml").unwrap();
    let configuration: Configuration = serde_yaml::from_str(&yaml).unwrap();
    fs::create_dir_all("maps/").unwrap();
    fs::write("maps/congresentatives.svg", format!(
        "\
<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">
    <title>{}</title>
    <style>
{}
    </style>
</svg>
        ",
        configuration.bounding_box.right - configuration.bounding_box.left,
        configuration.bounding_box.bottom - configuration.bounding_box.top,
        configuration.title, configuration.style,
    )).unwrap();
}

#[derive(Deserialize)]
struct Configuration {
    title: String,
    style: String,
    bounding_box: Box,
}

#[derive(Deserialize)]
struct Box {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}
