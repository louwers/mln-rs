// Ports test/sprite/sprite_parser.test.cpp.

use std::collections::BTreeSet;

use mln_platform_default_image::decode_image;
use mln_style_types::image::{ImageContent, TextFit};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, EventSeverity};

use super::{FixtureLog, read_file};
use crate::sprite_parser::{create_style_image, parse_sprite};

fn read_image(name: &str) -> PremultipliedImage {
    decode_image(&read_file(name)).unwrap()
}

fn create(
    image: &PremultipliedImage,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    ratio: f64,
) -> Option<mln_style_types::image::Image> {
    create_style_image("test", image, x, y, w, h, ratio, false, Vec::new(), Vec::new(), None, None, None)
}

fn sprite_error(log: &FixtureLog, msg: &str) -> usize {
    log.count(EventSeverity::Error, Event::Sprite, -1, msg)
}

fn sprite_warning(log: &FixtureLog, msg: &str) -> usize {
    log.count(EventSeverity::Warning, Event::Sprite, -1, msg)
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
    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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
    .map(|s| s.to_string())
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
    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");

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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");

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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");

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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, br#"{ "image": {} }"#).unwrap();
    assert_eq!(0, images.len());

    assert_eq!(1, sprite_error(&log, "Can't create image with invalid metrics: 0x0@0,0 in 200x299@1x sprite"));
}

// gtest: Sprite.SpriteParsingSimpleWidthHeight
#[test]
fn sprite_parsing_simple_width_height() {
    let _log = FixtureLog::new();

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
    let images = parse_sprite("default", &image_1x, br#"{ "image": { "width": 32, "height": 32 } }"#).unwrap();
    assert_eq!(1, images.len());
}

// gtest: Sprite.SpriteParsingWidthTooBig
#[test]
fn sprite_parsing_width_too_big() {
    let log = FixtureLog::new();

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");
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

    let image_1x = read_file("test/fixtures/annotations/emerald.png");

    parse_sprite("default", &image_1x, &text_fit_json(r#""textFitWidth": "invalid", "textFitHeight": "stretchOnly""#))
        .unwrap();
    assert_eq!(
        1,
        sprite_warning(&log, "Invalid sprite image 'image': value of 'textFitWidth' is an invalid value 'invalid'")
    );

    parse_sprite("default", &image_1x, &text_fit_json(r#""textFitHeight": 0"#)).unwrap();
    assert_eq!(1, sprite_warning(&log, "Invalid sprite image 'image': value of 'textFitHeight' must be a string"));
}
