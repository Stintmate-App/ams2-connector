//! AMS2 ids and display names, and the `catalog` messages built from them.
//!
//! Ids (opaque to Pitwall, kept exactly as the recorder has always stored them):
//!
//! * `car_id` = AMS2's car name (`mCarName`, e.g. `Ginetta G40 Cup`);
//! * `class_id` = AMS2's class code (`mCarClassName`, e.g. `G40Cup`);
//! * `track_id` = the track location (`mTrackLocation`, e.g. `OultonPark`);
//! * `layout_id` = the track variation (`mTrackVariation`, e.g. `OultonPark_International`).
//!
//! Display names, first match wins: `names.json` (embedded: `classes`, `cars`, `tracks`, and
//! `layouts` keyed `"<track_id>/<layout_id>"`); then a fallback: a car's name is its id, anything
//! else is the id with `_` as spaces and CamelCase split; a layout whose id is its track's id
//! gets the track's name.

use crate::messages::{Car, CarClass, Catalog, Layout, Track};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

/// The embedded `names.json`.
pub const NAMES_JSON: &str = include_str!("../data/names.json");

/// `OultonPark_International` to `Oulton Park International`; `G40Cup` to `G40 Cup`; codes such
/// as `TC60S2` stay as they are.
pub fn split_name(raw: &str) -> String {
    let chars: Vec<char> = raw.replace('_', " ").chars().collect();
    // lower -> Upper
    let mut first = String::with_capacity(raw.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && chars[i - 1].is_ascii_lowercase() && c.is_ascii_uppercase() {
            first.push(' ');
        }
        first.push(c);
    }
    // digit -> Upper followed by lower
    let chars: Vec<char> = first.chars().collect();
    let mut second = String::with_capacity(first.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0
            && chars[i - 1].is_ascii_digit()
            && c.is_ascii_uppercase()
            && chars.get(i + 1).is_some_and(char::is_ascii_lowercase)
        {
            second.push(' ');
        }
        second.push(c);
    }
    let joined = second.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        raw.to_owned()
    } else {
        joined
    }
}

/// Display names for AMS2 ids.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Names {
    /// Class id to name.
    #[serde(default)]
    pub classes: HashMap<String, String>,
    /// Car id to name.
    #[serde(default)]
    pub cars: HashMap<String, String>,
    /// Track id to name.
    #[serde(default)]
    pub tracks: HashMap<String, String>,
    /// `"<track_id>/<layout_id>"` to name.
    #[serde(default)]
    pub layouts: HashMap<String, String>,
}

impl Names {
    /// Parses a `names.json` text.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The embedded names.
    pub fn embedded() -> &'static Names {
        static NAMES: OnceLock<Names> = OnceLock::new();
        NAMES.get_or_init(|| Names::from_json(NAMES_JSON).expect("embedded names.json is valid"))
    }

    /// Display name of a class.
    pub fn class_name(&self, class_id: &str) -> String {
        self.classes
            .get(class_id)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| split_name(class_id))
    }

    /// Display name of a car.
    pub fn car_name(&self, car_id: &str) -> String {
        self.cars
            .get(car_id)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| car_id.to_owned())
    }

    /// Display name of a track.
    pub fn track_name(&self, track_id: &str) -> String {
        self.tracks
            .get(track_id)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| split_name(track_id))
    }

    /// Display name of a layout.
    pub fn layout_name(&self, track_id: &str, layout_id: &str) -> String {
        if let Some(n) = self
            .layouts
            .get(&format!("{track_id}/{layout_id}"))
            .filter(|s| !s.is_empty())
        {
            return n.clone();
        }
        if layout_id == track_id {
            return self.track_name(track_id);
        }
        split_name(layout_id)
    }
}

/// The catalogue ids of one AMS2 session (the player's car and the layout).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionIds {
    /// Car id.
    pub car_id: String,
    /// Class id (an empty class is no class).
    pub class_id: Option<String>,
    /// Track id.
    pub track_id: String,
    /// Layout id.
    pub layout_id: String,
    /// Layout length.
    pub length_m: Option<f64>,
    /// The player's car's tank.
    pub fuel_capacity_l: Option<f64>,
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x > 0.0)
}

/// Ids from AMS2's own strings.
pub fn session_ids(
    car: &str,
    car_class: Option<&str>,
    location: &str,
    variation: &str,
    length_m: Option<f64>,
    fuel_capacity_l: Option<f64>,
) -> SessionIds {
    SessionIds {
        car_id: car.to_owned(),
        class_id: car_class.filter(|c| !c.is_empty()).map(str::to_owned),
        track_id: location.to_owned(),
        layout_id: variation.to_owned(),
        length_m: positive(length_m),
        fuel_capacity_l: positive(fuel_capacity_l),
    }
}

/// A `catalog` message naming every class, car, track and layout of `sessions`, plus extra
/// `(car_id, class_id)` pairs (e.g. rivals' cars). Duplicates are sent once; a layout keeps the
/// first known length; a car's tank capacity is the last one given.
pub fn catalog(
    sessions: &[SessionIds],
    cars: &[(String, Option<String>)],
    names: &Names,
) -> Catalog {
    let mut car_classes: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut capacities: HashMap<String, f64> = HashMap::new();
    let mut layouts: BTreeMap<(String, String), Option<f64>> = BTreeMap::new();
    for s in sessions {
        let slot = car_classes
            .entry(s.car_id.clone())
            .or_insert_with(|| s.class_id.clone());
        if slot.is_none() {
            *slot = s.class_id.clone();
        }
        if let Some(c) = s.fuel_capacity_l {
            capacities.insert(s.car_id.clone(), c);
        }
        let len = layouts
            .entry((s.track_id.clone(), s.layout_id.clone()))
            .or_insert(None);
        if len.is_none() {
            *len = s.length_m;
        }
    }
    for (car_id, class_id) in cars {
        if car_id.is_empty() {
            continue;
        }
        let class_id = class_id.clone().filter(|c| !c.is_empty());
        let slot = car_classes
            .entry(car_id.clone())
            .or_insert_with(|| class_id.clone());
        if slot.is_none() {
            *slot = class_id;
        }
    }
    let class_ids: BTreeSet<&String> = car_classes.values().flatten().collect();
    let track_ids: BTreeSet<&String> = layouts.keys().map(|(t, _)| t).collect();
    Catalog {
        classes: class_ids
            .into_iter()
            .map(|c| CarClass {
                class_id: c.clone(),
                name: names.class_name(c),
            })
            .collect(),
        cars: car_classes
            .iter()
            .map(|(id, class)| Car {
                car_id: id.clone(),
                name: names.car_name(id),
                class_id: class.clone(),
                fuel_capacity_l: capacities.get(id).copied(),
            })
            .collect(),
        tracks: track_ids
            .into_iter()
            .map(|t| Track {
                track_id: t.clone(),
                name: names.track_name(t),
            })
            .collect(),
        layouts: layouts
            .iter()
            .map(|((t, y), len)| Layout {
                track_id: t.clone(),
                layout_id: y.clone(),
                name: names.layout_name(t, y),
                length_m: *len,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_names() {
        assert_eq!(
            split_name("OultonPark_International"),
            "Oulton Park International"
        );
        assert_eq!(split_name("G40Cup"), "G40 Cup");
        assert_eq!(split_name("TC60S2"), "TC60S2");
        assert_eq!(split_name("Road_Atlanta"), "Road Atlanta");
        assert_eq!(split_name("_"), "_");
    }

    #[test]
    fn embedded_names_and_fallbacks() {
        let n = Names::embedded();
        assert_eq!(n.class_name("G40Cup"), "Ginetta G40 Cup");
        assert_eq!(n.class_name("Other"), "Other");
        assert_eq!(n.car_name("MINI Cooper S 1965"), "MINI Cooper S 1965");
        assert_eq!(
            n.layout_name("OultonPark", "OultonPark_International"),
            "Oulton Park International"
        );
        assert_eq!(
            n.layout_name("Road_Atlanta", "Road_Atlanta"),
            "Road Atlanta (full course)"
        );
        assert_eq!(n.layout_name("Spa", "Spa"), "Spa");
        assert_eq!(n.track_name("OultonPark"), "Oulton Park");
    }

    #[test]
    fn catalog_dedups_and_sorts() {
        let n = Names::embedded();
        let a = session_ids("B Car", Some("G40Cup"), "Spa", "Spa", None, Some(50.0));
        let b = session_ids("A Car", None, "Spa", "Spa_Short", Some(3000.0), None);
        let c = session_ids("B Car", None, "Spa", "Spa", Some(7000.0), Some(60.0));
        let cat = catalog(&[a, b, c], &[("A Car".into(), Some("G40Cup".into()))], n);
        assert_eq!(cat.classes.len(), 1);
        assert_eq!(cat.cars[0].car_id, "A Car");
        assert_eq!(cat.cars[0].class_id.as_deref(), Some("G40Cup"));
        assert_eq!(cat.cars[1].fuel_capacity_l, Some(60.0));
        assert_eq!(cat.layouts.len(), 2);
        assert_eq!(cat.layouts[0].layout_id, "Spa");
        assert_eq!(cat.layouts[0].length_m, Some(7000.0));
        assert_eq!(cat.tracks.len(), 1);
    }
}
