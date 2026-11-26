use std::collections::BTreeSet;

use mln_image::decode_image;
use mln_style::bitmap::PremultipliedImage;
use mln_style::image::{ImageContent, TextFit};

use super::{FixtureLog, read_file};
use crate::sprite_parser::{create_style_image, parse_sprite};

fn read_image(name: &str) -> PremultipliedImage {
    decode_image(&read_file(name)).unwrap()
}

fn create(image: &PremultipliedImage, x: i32, y: i32, w: i32, h: i32, ratio: f64) -> Option<mln_style::image::Image> {
    create_style_image("test", image, x, y, w, h, ratio, false, Vec::new(), Vec::new(), None, None, None)
}

fn sprite_error(log: &FixtureLog, msg: &str) -> usize {
    log.count(log::Level::Error, crate::log_target::SPRITE, msg)
}

fn sprite_warning(log: &FixtureLog, msg: &str) -> usize {
    log.count(log::Level::Warn, crate::log_target::SPRITE, msg)
}

// gtest: Sprite.SpriteImageCreationInvalid
#[test]
fn sprite_image_creation_invalid() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    assert_eq!(200, image_1x.size.width);
    assert_eq!(299, image_1x.size.height);

    let cases: [(i32, i32, i32, i32, f64, &str); 13] = [
        (0, 0, 0, 16, 1.0, "0x16@0,0 in 200x299@1x"),       // width == 0
        (0, 0, 16, 0, 1.0, "16x0@0,0 in 200x299@1x"),       // height == 0
        (0, 0, -1, 16, 1.0, "-1x16@0,0 in 200x299@1x"),     // width < 0
        (0, 0, 16, -1, 1.0, "16x-1@0,0 in 200x299@1x"),     // height < 0
        (0, 0, 1, 1, 0.0, "1x1@0,0 in 200x299@0x"),         // ratio == 0
        (0, 0, 1, 1, -1.0, "1x1@0,0 in 200x299@-1x"),       // ratio < 0
        (0, 0, 1, 1, 23.0, "1x1@0,0 in 200x299@23x"),       // ratio too large
        (0, 0, 2048, 16, 1.0, "2048x16@0,0 in 200x299@1x"), // too wide
        (0, 0, 16, 1025, 1.0, "16x1025@0,0 in 200x299@1x"), // too tall
        (-1, 0, 16, 16, 1.0, "16x16@-1,0 in 200x299@1x"),   // srcX < 0
        (0, -1, 16, 16, 1.0, "16x16@0,-1 in 200x299@1x"),   // srcY < 0
        (0, 0, 201, 16, 1.0, "201x16@0,0 in 200x299@1x"),   // right edge out of bounds
        (0, 0, 16, 300, 1.0, "16x300@0,0 in 200x299@1x"),   // bottom edge out of bounds
    ];
    for (x, y, w, h, ratio, metrics) in cases {
        assert!(create(&image_1x, x, y, w, h, ratio).is_none(), "{metrics}");
        assert_eq!(
            1,
            sprite_error(&log, &format!("Can't create image with invalid metrics: {metrics} sprite")),
            "{metrics}"
        );
        assert_eq!(0, log.unchecked_count(), "{metrics}");
    }

    assert!(
        create_style_image(
            "test",
            &image_1x,
            0,
            0,
            16,
            16,
            1.0,
            false,
            vec![(-1.0, 3.0)],
            Vec::new(),
            None,
            None,
            None
        )
        .is_none()
    );
    assert_eq!(
        1,
        sprite_error(&log, "Can't create image with invalid metadata: stretchX is out of bounds or overlapping")
    );
    assert_eq!(0, log.unchecked_count());

    let content = ImageContent { left: 14.0, top: 14.0, right: 12.0, bottom: 10.0 };
    assert!(
        create_style_image(
            "test",
            &image_1x,
            0,
            0,
            16,
            16,
            1.0,
            false,
            Vec::new(),
            Vec::new(),
            Some(content),
            None,
            None
        )
        .is_none()
    );
    assert_eq!(1, sprite_error(&log, "Can't create image with invalid metadata: content area is invalid"));
    assert_eq!(0, log.unchecked_count());
}

// gtest: Sprite.SpriteImageCreation1x
#[test]
fn sprite_image_creation_1x() {
    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    assert_eq!(200, image_1x.size.width);
    assert_eq!(299, image_1x.size.height);

    // "museum_icon":{"x":177,"y":187,"width":18,"height":18,"pixelRatio":1,"sdf":false}
    let sprite = create(&image_1x, 177, 187, 18, 18, 1.0).unwrap();
    assert_eq!(18, sprite.get_image().size.width);
    assert_eq!(18, sprite.get_image().size.height);
    assert_eq!(1.0, sprite.get_pixel_ratio());
    assert_eq!(&read_image("test/fixtures/annotations/result-spriteimagecreation1x-museum.png"), sprite.get_image());
}

// gtest: Sprite.SpriteImageCreation2x
#[test]
fn sprite_image_creation_2x() {
    let image_2x = read_image("test/fixtures/annotations/emerald@2x.png");

    // "museum_icon":{"x":354,"y":374,"width":36,"height":36,"pixelRatio":2,"sdf":false}
    let sprite = create(&image_2x, 354, 374, 36, 36, 2.0).unwrap();
    assert_eq!(36, sprite.get_image().size.width);
    assert_eq!(36, sprite.get_image().size.height);
    assert_eq!(2.0, sprite.get_pixel_ratio());
    assert_eq!(&read_image("test/fixtures/annotations/result-spriteimagecreation2x.png"), sprite.get_image());
}

// gtest: Sprite.SpriteImageCreation1_5x
#[test]
fn sprite_image_creation_1_5x() {
    let image_2x = read_image("test/fixtures/annotations/emerald@2x.png");

    // "museum_icon":{"x":354,"y":374,"width":36,"height":36,"pixelRatio":2,"sdf":false}
    let sprite = create(&image_2x, 354, 374, 36, 36, 1.5).unwrap();
    assert_eq!(36, sprite.get_image().size.width);
    assert_eq!(36, sprite.get_image().size.height);
    assert_eq!(1.5, sprite.get_pixel_ratio());
    assert_eq!(&read_image("test/fixtures/annotations/result-spriteimagecreation1_5x-museum.png"), sprite.get_image());

    // "hospital_icon":{"x":314,"y":518,"width":36,"height":36,"pixelRatio":2,"sdf":false}
    let sprite2 = create(&image_2x, 314, 518, 35, 35, 1.5).unwrap();
    assert_eq!(35, sprite2.get_image().size.width);
    assert_eq!(35, sprite2.get_image().size.height);
    assert_eq!(1.5, sprite2.get_pixel_ratio());
    assert_eq!(
        &read_image("test/fixtures/annotations/result-spriteimagecreation1_5x-hospital.png"),
        sprite2.get_image()
    );
}

// gtest: Sprite.SpriteParsing
#[test]
fn sprite_parsing() {
    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let json_1x = read_file("test/fixtures/annotations/emerald.json");

    let images = parse_sprite("default", &image_1x, &json_1x).unwrap();

    let names: BTreeSet<String> = images.iter().map(|image| image.id.clone()).collect();

    let expected: BTreeSet<String> = [
        "airfield_icon",
        "airport_icon",
        "background",
        "cemetery_icon",
        "college_icon",
        "default_1",
        "default_2",
        "default_3",
        "default_4",
        "default_5",
        "default_6",
        "default_marker",
        "dlr",
        "dlr.london-overground.london-underground.national-rail",
        "dlr.london-underground",
        "dlr.london-underground.national-rail",
        "dlr.national-rail",
        "dot",
        "embassy_icon",
        "fire-station_icon",
        "generic-metro",
        "generic-rail",
        "generic_icon",
        "golf_icon",
        "government_icon",
        "grass_pattern",
        "harbor_icon",
        "hospital_icon",
        "hospital_striped",
        "interstate_1",
        "interstate_2",
        "interstate_3",
        "library_icon",
        "london-overground",
        "london-overground.london-underground",
        "london-overground.london-underground.national-rail",
        "london-overground.national-rail",
        "london-underground",
        "london-underground.national-rail",
        "marker_icon",
        "metro",
        "metro.rer",
        "monument_icon",
        "moscow-metro",
        "museum_icon",
        "national-rail",
        "oneway_motorway",
        "oneway_road",
        "park_icon",
        "police_icon",
        "post_icon",
        "prison_icon",
        "religious-christian_icon",
        "religious-jewish_icon",
        "religious-muslim_icon",
        "rer",
        "rer.transilien",
        "s-bahn",
        "s-bahn.u-bahn",
        "sand_noise",
        "school_icon",
        "school_striped",
        "secondary_marker",
        "u-bahn",
        "us_highway_1",
        "us_highway_2",
        "us_highway_3",
        "us_state_1",
        "us_state_2",
        "us_state_3",
        "washington-metro",
        "wiener-linien",
        "zoo_icon",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(expected, names);

    let sprite = images.iter().find(|image| image.id == "generic-metro").unwrap();
    assert_eq!(18, sprite.image.size.width);
    assert_eq!(18, sprite.image.size.height);
    assert_eq!(1.0, sprite.pixel_ratio);
    assert_eq!(read_image("test/fixtures/annotations/result-spriteparsing.png"), sprite.image);
}

// gtest: Sprite.SpriteParsingInvalidJSON
#[test]
fn sprite_parsing_invalid_json() {
    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let json_1x = br#"{ "image": " }"#;

    let err = parse_sprite("default", &image_1x, json_1x).expect_err("Expected exception");
    // C++: "Failed to parse JSON: Missing a closing quotation mark in string.
    // at offset 14" (rapidjson's message).
    assert!(err.to_string().starts_with("Failed to parse JSON: "), "{err}");
}

fn interstate(extra: &str) -> Vec<u8> {
    format!(
        r#"{{
            "interstate_1": {{
                "width": 40,
                "height": 42,
                "x": 100,
                "y": 44,
                "pixelRatio": 1,
                {extra}
            }}
        }}"#
    )
    .into_bytes()
}

// gtest: Sprite.SpriteParsingInvalidStretches
#[test]
fn sprite_parsing_invalid_stretches() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    parse_sprite("default", &image_1x, &interstate(r#""stretchX": {}"#)).unwrap();
    assert_eq!(1, sprite_warning(&log, "Invalid sprite image 'interstate_1': value of 'stretchX' must be an array"));
    assert_eq!(0, log.unchecked_count());

    parse_sprite("default", &image_1x, &interstate(r#""stretchX": [[[]]]"#)).unwrap();
    assert_eq!(
        1,
        sprite_warning(
            &log,
            "Invalid sprite image 'interstate_1': members of 'stretchX' must be an array of two numbers"
        )
    );
    assert_eq!(0, log.unchecked_count());

    parse_sprite("default", &image_1x, &interstate(r#""stretchX": [[4, false]]"#)).unwrap();
    assert_eq!(
        1,
        sprite_warning(
            &log,
            "Invalid sprite image 'interstate_1': members of 'stretchX' must be an array of two numbers"
        )
    );
    assert_eq!(0, log.unchecked_count());
}

// gtest: Sprite.SpriteParsingInvalidContent
#[test]
fn sprite_parsing_invalid_content() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    parse_sprite("default", &image_1x, &interstate(r#""content": []"#)).unwrap();
    assert_eq!(
        1,
        sprite_warning(
            &log,
            "Invalid sprite image 'interstate_1': value of 'content' must be an array of four numbers"
        )
    );
    assert_eq!(0, log.unchecked_count());

    parse_sprite("default", &image_1x, &interstate(r#""content": [4, 4, 4, true]"#)).unwrap();
    assert_eq!(
        1,
        sprite_warning(
            &log,
            "Invalid sprite image 'interstate_1': value of 'content' must be an array of four numbers"
        )
    );
    assert_eq!(0, log.unchecked_count());
}

// gtest: Sprite.SpriteParsingStretchAndContent
#[test]
fn sprite_parsing_stretch_and_content() {
    let _log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    let images = parse_sprite(
        "default",
        &image_1x,
        br#"{
        "image": {
            "width": 16,
            "height": 16,
            "x": 0,
            "y": 0,
            "pixelRatio": 1,
            "stretchX": [[2, 14]],
            "stretchY": [[0, 4], [12, 16]],
            "content": [2, 2, 14, 14]
        }
    }"#,
    )
    .unwrap();
    assert_eq!(1, images.len());
    assert_eq!("image", images[0].id);
    assert_eq!(vec![(2.0f32, 14.0f32)], images[0].stretch_x);
    assert_eq!(vec![(0.0f32, 4.0f32), (12.0f32, 16.0f32)], images[0].stretch_y);
    assert_eq!(Some(ImageContent { left: 2.0, top: 2.0, right: 14.0, bottom: 14.0 }), images[0].content);
}

// gtest: Sprite.SpriteParsingEmptyImage
#[test]
fn sprite_parsing_empty_image() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, br#"{ "image": {} }"#).unwrap();
    assert_eq!(0, images.len());

    assert_eq!(1, sprite_error(&log, "Can't create image with invalid metrics: 0x0@0,0 in 200x299@1x sprite"));
}

// gtest: Sprite.SpriteParsingSimpleWidthHeight
#[test]
fn sprite_parsing_simple_width_height() {
    let _log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, br#"{ "image": { "width": 32, "height": 32 } }"#).unwrap();
    assert_eq!(1, images.len());
}

// gtest: Sprite.SpriteParsingWidthTooBig
#[test]
fn sprite_parsing_width_too_big() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, br#"{ "image": { "width": 65536, "height": 32 } }"#).unwrap();
    assert_eq!(0, images.len());

    assert_eq!(
        1,
        sprite_warning(&log, "Invalid sprite image 'image': value of 'width' must be an integer between 0 and 65535")
    );
    assert_eq!(1, sprite_error(&log, "Can't create image with invalid metrics: 0x32@0,0 in 200x299@1x sprite"));
}

// gtest: Sprite.SpriteParsingNegativeWidth
#[test]
fn sprite_parsing_negative_width() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, br#"{ "image": { "width": -1, "height": 32 } }"#).unwrap();
    assert_eq!(0, images.len());

    assert_eq!(
        1,
        sprite_warning(&log, "Invalid sprite image 'image': value of 'width' must be an integer between 0 and 65535")
    );
    assert_eq!(1, sprite_error(&log, "Can't create image with invalid metrics: 0x32@0,0 in 200x299@1x sprite"));
}

// gtest: Sprite.SpriteParsingNullRatio
#[test]
fn sprite_parsing_null_ratio() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images =
        parse_sprite("default", &image_1x, br#"{ "image": { "width": 32, "height": 32, "pixelRatio": 0 } }"#).unwrap();
    assert_eq!(0, images.len());

    assert_eq!(1, sprite_error(&log, "Can't create image with invalid metrics: 32x32@0,0 in 200x299@0x sprite"));
}

fn text_fit_json(extra: &str) -> Vec<u8> {
    format!(
        r#"{{
        "image": {{
            "width": 16,
            "height": 16,
            "x": 0,
            "y": 0,
            "pixelRatio": 1,
            {extra}
        }}
    }}"#
    )
    .into_bytes()
}

// gtest: Sprite.SpriteParsingTextFit
#[test]
fn sprite_parsing_text_fit() {
    let _log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite(
        "default",
        &image_1x,
        &text_fit_json(r#""textFitWidth": "stretchOrShrink", "textFitHeight": "proportional""#),
    )
    .unwrap();
    assert_eq!(1, images.len());
    assert_eq!("image", images[0].id);
    assert_eq!(Some(TextFit::StretchOrShrink), images[0].text_fit_width);
    assert_eq!(Some(TextFit::Proportional), images[0].text_fit_height);
}

// gtest: Sprite.SpriteParsingNullTextFit
#[test]
fn sprite_parsing_null_text_fit() {
    let _log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, &text_fit_json(r#""textFitHeight": "stretchOnly""#)).unwrap();
    assert_eq!(1, images.len());
    assert_eq!("image", images[0].id);
    assert!(images[0].text_fit_width.is_none());
    assert_eq!(Some(TextFit::StretchOnly), images[0].text_fit_height);
}

// gtest: Sprite.SpriteParsingInvalidTextFit
#[test]
fn sprite_parsing_invalid_text_fit() {
    let log = FixtureLog::new();

    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    parse_sprite("default", &image_1x, &text_fit_json(r#""textFitWidth": "invalid", "textFitHeight": "stretchOnly""#))
        .unwrap();
    assert_eq!(
        1,
        sprite_warning(&log, "Invalid sprite image 'image': value of 'textFitWidth' is an invalid value 'invalid'")
    );

    parse_sprite("default", &image_1x, &text_fit_json(r#""textFitHeight": 0"#)).unwrap();
    assert_eq!(1, sprite_warning(&log, "Invalid sprite image 'image': value of 'textFitHeight' must be a string"));
}

#[test]
fn sprite_parsing_warns_about_members_of_the_wrong_kind() {
    let log = FixtureLog::new();
    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    let cases = [
        (r#""width": 32.5"#, "value of 'width' must be an integer between 0 and 65535"),
        (r#""x": "1""#, "value of 'x' must be an integer between 0 and 65535"),
        (r#""y": 1e3"#, "value of 'y' must be an integer between 0 and 65535"),
        (r#""height": 18446744073709551616"#, "value of 'height' must be an integer between 0 and 65535"),
        (r#""pixelRatio": "2""#, "value of 'pixelRatio' must be a number"),
        (r#""pixelRatio": null"#, "value of 'pixelRatio' must be a number"),
        (r#""sdf": 1"#, "value of 'sdf' must be a boolean"),
        (r#""stretchY": [[1, 2, 3]]"#, "members of 'stretchY' must be an array of two numbers"),
        (r#""stretchY": "[[1, 2]]""#, "value of 'stretchY' must be an array"),
        (r#""content": [1, 2, 3, 4, 5]"#, "value of 'content' must be an array of four numbers"),
        (r#""content": {"left": 1}"#, "value of 'content' must be an array of four numbers"),
        (r#""textFitWidth": null"#, "value of 'textFitWidth' must be a string"),
    ];
    for (member, message) in cases {
        parse_sprite("default", &image_1x, &interstate(member)).unwrap();
        let message = format!("Invalid sprite image 'interstate_1': {message}");
        assert_eq!(1, sprite_warning(&log, &message), "{member}");
        // A width or height of the wrong kind is 0, which the metrics reject.
        let rejected = if member.contains("width") {
            Some("0x42@100,44")
        } else if member.contains("height") {
            Some("40x0@100,44")
        } else {
            None
        };
        if let Some(metrics) = rejected {
            let error = format!("Can't create image with invalid metrics: {metrics} in 200x299@1x sprite");
            assert_eq!(1, sprite_error(&log, &error), "{member}");
        }
        assert_eq!(0, log.unchecked_count(), "{member}");
    }
}

#[test]
fn sprite_parsing_keeps_the_valid_members_of_a_lenient_entry() {
    let log = FixtureLog::new();
    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    let json = interstate(r#""sdf": "yes", "stretchX": [[2, 4], [6], [8.5, 10]], "content": [1, 2, 3, 4.25]"#);
    let images = parse_sprite("default", &image_1x, &json).unwrap();
    assert_eq!(1, images.len());
    let image = &images[0];
    assert!(!image.sdf);
    assert_eq!(vec![(2.0f32, 4.0f32), (8.5f32, 10.0f32)], image.stretch_x);
    assert_eq!(Some(ImageContent { left: 1.0, top: 2.0, right: 3.0, bottom: 4.25 }), image.content);
    assert_eq!(1, sprite_warning(&log, "Invalid sprite image 'interstate_1': value of 'sdf' must be a boolean"));
    assert_eq!(
        1,
        sprite_warning(
            &log,
            "Invalid sprite image 'interstate_1': members of 'stretchX' must be an array of two numbers"
        )
    );
    assert_eq!(0, log.unchecked_count());
}

#[test]
fn sprite_parsing_skips_entries_that_arent_objects() {
    let log = FixtureLog::new();
    let image_1x = read_image("test/fixtures/annotations/emerald.png");

    let json = br#"{ "a": 1, "b": [], "c": null, "image": { "width": 16, "height": 16 } }"#;
    let images = parse_sprite("default", &image_1x, json).unwrap();
    assert_eq!(["image"], images.iter().map(|image| image.id.as_str()).collect::<Vec<_>>().as_slice());
    assert_eq!(0, log.unchecked_count());
}

#[test]
fn sprite_parsing_names_the_images_of_another_sprite_by_its_id() {
    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("extra", &image_1x, br#"{ "image": { "width": 16, "height": 16 } }"#).unwrap();
    assert_eq!("extra:image", images[0].id);
}

#[test]
fn sprite_parsing_fails_when_the_root_isnt_an_object() {
    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    for json in [&b"[]"[..], b"null", b"1"] {
        let err = parse_sprite("default", &image_1x, json).expect_err("a root that isn't an object");
        assert_eq!("Sprite JSON root must be an object", err.message, "{}", String::from_utf8_lossy(json));
    }
}

#[test]
fn sprite_parsing_takes_the_last_of_duplicate_entries() {
    let image_1x = read_image("test/fixtures/annotations/emerald.png");
    let json = br#"{ "image": { "width": 16, "height": 16 }, "image": { "width": 8, "height": 8 } }"#;
    let images = parse_sprite("default", &image_1x, json).unwrap();
    assert_eq!(1, images.len());
    assert_eq!(8, images[0].image.size.width);
}
