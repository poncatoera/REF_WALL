use wall_format::{ItemKind, Look};

#[test]
fn plain_look_is_left_out_and_set_fields_round_trip() {
    let hash = "ab".repeat(32).parse().unwrap();
    let plain = ItemKind::Media { hash, crop: None, look: Look::default() };
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("look"), "{json}");
    assert_eq!(serde_json::from_str::<ItemKind>(&json).unwrap(), plain);

    let ItemKind::Media { hash, .. } = plain else { unreachable!() };
    let turned = ItemKind::Media { hash, crop: None, look: Look { turns: 3, mirror: true, ..Default::default() } };
    let json = serde_json::to_string(&turned).unwrap();
    assert!(json.contains(r#""look":{"turns":3,"mirror":true}"#), "{json}");
    assert_eq!(serde_json::from_str::<ItemKind>(&json).unwrap(), turned);
}
