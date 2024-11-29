use anyhow::{anyhow, Result};
use shapefile::Shape;
use shapefile::dbase::FieldValue;


static STATES_OF_INTEREST: [&str; 13] = ["ME", "NH", "VT", "MA", "RI", "CT", "NY", "NJ", "PA", "DE", "MD", "DC", "VA"];
static FILENAME: &str = "data/USA_Major_Cities.shp";
static NAME_KEY: &str = "NAME";
static STATE_NAME_KEY: &str = "STATE_ABBR";
static POPULATION_KEY: &str = "POPULATION";
static CAPITAL_KEY: &str = "CAPITAL";
static ID_KEY: &str = "PLACE_FIPS";


fn main() -> Result<()> {
    let mut reader = shapefile::Reader::from_path(FILENAME).or(Err(anyhow!("could not find `{}`", &FILENAME)))?;

    let mut cities = Vec::new();

    for shape_record in reader.iter_shapes_and_records().into_iter() {
        let (shape, record) = shape_record?;
        let name = match record.get(NAME_KEY) {
            Some(FieldValue::Character(Some(string))) => string,
            Some(FieldValue::Character(None)) => continue,
            Some(other) => return Err(anyhow!("the {} column must be strings, but I found {}", NAME_KEY, other.field_type())),
            None => return Err(anyhow!("I couldn't find the {} column.", NAME_KEY)),
        };
        let state_name = match record.get(STATE_NAME_KEY) {
            Some(FieldValue::Character(Some(string))) => string,
            Some(FieldValue::Character(None)) => continue,
            Some(other) => return Err(anyhow!("the {} column must be strings, but I found {}", STATE_NAME_KEY, other.field_type())),
            None => return Err(anyhow!("I couldn't find the {} column.", STATE_NAME_KEY)),
        };
        let population = match record.get(POPULATION_KEY) {
            Some(FieldValue::Numeric(Some(number))) => *number as u32,
            Some(FieldValue::Numeric(None)) => continue,
            Some(other) => return Err(anyhow!("the {} column must be numeric, but I found {}", POPULATION_KEY, other.field_type())),
            None => return Err(anyhow!("I couldn't find the {} column.", POPULATION_KEY)),
        };
        let is_capital = match record.get(CAPITAL_KEY) {
            Some(FieldValue::Character(Some(_))) => true,
            Some(FieldValue::Character(None)) => false,
            Some(other) => return Err(anyhow!("I expected the {} column to be strings, but I found {}", CAPITAL_KEY, other.field_type())),
            None => return Err(anyhow!("I couldn't find the {} column.", CAPITAL_KEY)),
        };
        let id = match record.get(ID_KEY) {
            Some(FieldValue::Character(Some(string))) => string,
            Some(FieldValue::Character(None)) => continue,
            Some(other) => return Err(anyhow!("the {} column must be numeric, but I found {}", ID_KEY, other.field_type())),
            None => return Err(anyhow!("I couldn't find the {} column.", ID_KEY)),
        };
        let location = match shape {
            Shape::Point(point) => point,
            other => return Err(anyhow!("the shapefile must be a POINT shapefile, but I found {}", other.shapetype())),
        };

        if STATES_OF_INTEREST.contains(&state_name.as_str()) {
            cities.push(City {
                name: name.to_string(),
                state_name: state_name.to_string(),
                population, is_capital,
                id: id.to_string(),
                latitude: location.y,
                longitude: location.x,
            });
        }
    }

    // sort them from biggest to smallest
    cities.sort_by(|a: &City, b: &City| u32::cmp(&a.population, &b.population).reverse());

    // go thru and choose which cities to keep and which to omit
    let mut chosen_cities = Vec::new();
    'new_city_loop:
    for new_city in cities.into_iter() {
        // re-sort this list every time because even if it starts out correct it might have changed last loop
        chosen_cities.sort_by(|a: &City, b: &City| u32::cmp(&a.population, &b.population).reverse());
        // look at all larger cities that are already on the map
        for big_city in &mut chosen_cities {
            match characterize_relationship(&big_city, &new_city) {
                // either deem that the larger city is irrelevant
                Relationship::ShowBoth => {
                },
                // or deem that the larger city is too close and this one must be omitted
                Relationship::ShowOne => {
                    continue 'new_city_loop;
                },
                // or deem that the larger city is so close that they should be treated as one
                Relationship::Merge => {
                    big_city.population += new_city.population;
                    continue 'new_city_loop;
                },
            }
        }
        // if they all turn up ShowBoth, then you may choose this city
        chosen_cities.push(new_city);
    }
    // finally, double-check that it's sorted right
    chosen_cities.sort_by(|a: &City, b: &City| u32::cmp(&a.population, &b.population).reverse());

    // print the result
    for city in &chosen_cities[..20] {
        println!("{}: {}, {}", city.id, city.name, city.state_name);
    }

    let mut city_id_list = Vec::new();
    for city in &chosen_cities {
        city_id_list.push(format!("\"{}\"", city.id));
    }
    println!("[{}]", city_id_list.join(", "));

    return Ok(());
}


fn characterize_relationship(metropole: &City, satellite: &City) -> Relationship {
    if satellite.is_capital {
        return Relationship::ShowBoth;
    }
    let distance = 6371.*f64::hypot(
        (metropole.latitude - satellite.latitude).to_radians(),
        (metropole.longitude - satellite.longitude).to_radians()*f64::cos(((metropole.latitude + satellite.latitude)/2.).to_radians()));
    let distance = if metropole.state_name.ne(&satellite.state_name) {
        distance + 6.
    }
    else {
        distance
    };
    let z_metropole = metropole.population as f64;
    let z_satellite = satellite.population as f64;
    let prominence = z_satellite - z_metropole*f64::exp(-(distance/8.).powi(2)/2.);
    if prominence < 0. {
        return Relationship::Merge;
    }
    else if prominence < 10_000. {
        return Relationship::ShowOne;
    }
    else {
        return Relationship::ShowBoth;
    }
}


#[derive(Debug)]
struct City {
    id: String,
    name: String,
    state_name: String,
    population: u32,
    is_capital: bool,
    latitude: f64,
    longitude: f64,
}


enum Relationship {
    Merge, ShowOne, ShowBoth,
}
