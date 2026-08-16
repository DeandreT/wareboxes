use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    CartonDimensions, CartonMeasurements, DimensionMillimeters, WeightGrams,
};

#[derive(Clone, Copy)]
pub(super) struct CartonMeasurementSignals {
    pub(super) weight: RwSignal<String>,
    pub(super) weight_automation_command_id: RwSignal<Option<i64>>,
    pub(super) scale_busy: RwSignal<bool>,
    pub(super) length: RwSignal<String>,
    pub(super) width: RwSignal<String>,
    pub(super) height: RwSignal<String>,
}

impl CartonMeasurementSignals {
    pub(super) fn clear(self) {
        self.weight.set(String::new());
        self.weight_automation_command_id.set(None);
        self.scale_busy.set(false);
        self.length.set(String::new());
        self.width.set(String::new());
        self.height.set(String::new());
    }
}

pub(super) fn parse_measurements(
    signals: CartonMeasurementSignals,
) -> Result<CartonMeasurements, String> {
    let weight = optional_positive(&signals.weight.get_untracked(), "weight")?
        .map(WeightGrams::new)
        .transpose()
        .map_err(|error| error.to_string())?;
    let dimensions = [
        signals.length.get_untracked(),
        signals.width.get_untracked(),
        signals.height.get_untracked(),
    ];
    let populated = dimensions
        .iter()
        .filter(|value| !value.trim().is_empty())
        .count();
    let dimensions = if populated == 0 {
        None
    } else if populated != 3 {
        return Err("Enter all three carton dimensions or leave all three blank.".to_owned());
    } else {
        let length_mm = DimensionMillimeters::new(required_positive(&dimensions[0], "length")?)
            .map_err(|error| error.to_string())?;
        let width_mm = DimensionMillimeters::new(required_positive(&dimensions[1], "width")?)
            .map_err(|error| error.to_string())?;
        let height_mm = DimensionMillimeters::new(required_positive(&dimensions[2], "height")?)
            .map_err(|error| error.to_string())?;
        Some(CartonDimensions {
            length_mm,
            width_mm,
            height_mm,
        })
    };
    Ok(CartonMeasurements {
        weight_grams: weight,
        dimensions,
    })
}

fn optional_positive(value: &str, label: &str) -> Result<Option<i64>, String> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        required_positive(value, label).map(Some)
    }
}

fn required_positive(value: &str, label: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Enter a positive whole-number {label}."))
}

#[cfg(test)]
mod tests {
    use super::required_positive;

    #[test]
    fn measurements_accept_only_positive_whole_numbers() {
        assert_eq!(required_positive(" 1250 ", "weight"), Ok(1250));
        assert!(required_positive("0", "weight").is_err());
        assert!(required_positive("1.5", "weight").is_err());
    }
}
