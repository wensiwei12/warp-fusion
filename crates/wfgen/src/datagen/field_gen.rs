use rand::Rng;
use rand::rngs::StdRng;
use wf_lang::FieldType;

/// Generate a default random value for a field type.
pub fn generate_field_value(field_type: &FieldType, rng: &mut StdRng) -> serde_json::Value {
    let base = match field_type {
        FieldType::Base(b) => b,
        FieldType::ArrayAny => return serde_json::Value::Array(Vec::new()),
        FieldType::Array(b) => {
            // Generate an array of 1-5 elements
            let len = rng.random_range(1..=5);
            let arr: Vec<serde_json::Value> =
                (0..len).map(|_| generate_default_base(b, rng)).collect();
            return serde_json::Value::Array(arr);
        }
        FieldType::Object => return serde_json::Value::Object(serde_json::Map::new()),
    };
    generate_default_base(base, rng)
}

fn generate_default_base(base: &wf_lang::BaseType, rng: &mut StdRng) -> serde_json::Value {
    use wf_lang::BaseType;
    match base {
        BaseType::Chars => {
            let len = rng.random_range(6..=16);
            let s: String = (0..len)
                .map(|_| {
                    let idx = rng.random_range(0..36u8);
                    if idx < 26 {
                        (b'a' + idx) as char
                    } else {
                        (b'0' + idx - 26) as char
                    }
                })
                .collect();
            serde_json::Value::String(s)
        }
        BaseType::Digit => {
            let n = rng.random_range(0..100_000i64);
            serde_json::Value::Number(serde_json::Number::from(n))
        }
        BaseType::Float => {
            let n: f64 = rng.random_range(0.0..1000.0);
            serde_json::json!(n)
        }
        BaseType::Bool => {
            let b: bool = rng.random();
            serde_json::Value::Bool(b)
        }
        BaseType::Time => {
            // Placeholder — actual timestamp is controlled by stream_gen
            serde_json::json!(0_i64)
        }
        BaseType::Ip => {
            // Random IPv4
            let a = rng.random_range(1..=254u8);
            let b = rng.random_range(0..=255u8);
            let c = rng.random_range(0..=255u8);
            let d = rng.random_range(1..=254u8);
            serde_json::Value::String(format!("{a}.{b}.{c}.{d}"))
        }
        BaseType::Hex => {
            let hex: String = (0..32)
                .map(|_| {
                    let idx = rng.random_range(0..16u8);
                    if idx < 10 {
                        (b'0' + idx) as char
                    } else {
                        (b'a' + idx - 10) as char
                    }
                })
                .collect();
            serde_json::Value::String(hex)
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use wf_lang::{BaseType, FieldType};

    use super::generate_field_value;

    #[test]
    fn structured_field_defaults_are_json_values() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);

        assert_eq!(
            generate_field_value(&FieldType::ArrayAny, &mut rng),
            serde_json::Value::Array(Vec::new())
        );
        assert_eq!(
            generate_field_value(&FieldType::Object, &mut rng),
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn typed_array_default_generates_array_values() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let value = generate_field_value(&FieldType::Array(BaseType::Digit), &mut rng);

        let arr = value.as_array().expect("typed array default");
        assert!(!arr.is_empty());
        assert!(arr.iter().all(|v| v.as_i64().is_some()));
    }
}
