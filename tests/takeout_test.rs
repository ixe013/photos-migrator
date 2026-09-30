use std::fs;

#[test]
fn test_takeout_matcher_and_scanner() {
    let root = std::env::temp_dir().join(format!("photos_migrator_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();

    let album_dir = root.join("Summer Trip 2022");
    fs::create_dir_all(&album_dir).unwrap();

    let year_dir = root.join("Photos from 2021");
    fs::create_dir_all(&year_dir).unwrap();

    // 1. Regular photo and sidecar
    let photo1 = year_dir.join("beach.jpg");
    fs::write(&photo1, b"sample image bytes 1").unwrap();
    let json1 = year_dir.join("beach.jpg.json");
    fs::write(
        &json1,
        r#"{
            "title": "beach.jpg",
            "description": "Sunny day",
            "photoTakenTime": {"timestamp": "1620000000"},
            "geoData": {"latitude": 34.0522, "longitude": -118.2437, "altitude": 10.0},
            "people": [{"name": "Alice"}, {"name": "Bob"}]
        }"#,
    ).unwrap();

    // 2. Truncated filename sidecar
    let long_photo = album_dir.join("a_very_long_file_name_that_exceeds_forty_five_characters_long_beach_walk.jpg");
    fs::write(&long_photo, b"sample image bytes 2").unwrap();
    let truncated_json = album_dir.join("a_very_long_file_name_that_exceeds_forty_five_ch.json");
    fs::write(
        &truncated_json,
        r#"{
            "title": "a_very_long_file_name_that_exceeds_forty_five_characters_long_beach_walk.jpg",
            "photoTakenTime": {"timestamp": "1650000000"}
        }"#,
    ).unwrap();

    // 3. Duplicate photo in album (same bytes as beach.jpg)
    let dup_photo = album_dir.join("beach_dup.jpg");
    fs::write(&dup_photo, b"sample image bytes 1").unwrap();

    // 4. Album metadata with collaborators
    let meta_json = album_dir.join("metadata.json");
    fs::write(
        &meta_json,
        r#"[{"displayName": "Alice Smith"}, {"displayName": "Charlie Brown"}]"#,
    ).unwrap();

    // Run TakeoutScanner
    let scanner = photos_migrator::takeout::TakeoutScanner::new(&root);
    let (pairs, album_collabs) = scanner.scan(true);

    assert_eq!(pairs.len(), 3);
    assert!(album_collabs.contains_key("Summer Trip 2022"));
    let collabs = &album_collabs["Summer Trip 2022"];
    assert!(collabs.contains(&"Alice Smith".to_string()));
    assert!(collabs.contains(&"Charlie Brown".to_string()));

    // Verify truncated sidecar was matched
    let long_pair = pairs.iter().find(|p| p.media_path == long_photo).unwrap();
    assert!(long_pair.sidecar_path.is_some());
    assert!(long_pair.metadata.is_some());

    // Clean up
    let _ = fs::remove_dir_all(&root);
}
