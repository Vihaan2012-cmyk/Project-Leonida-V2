use proptest::prelude::*;
use systems::Error;
use systems::rbf0::{self, Document, Element, Node, Value, ValueData, Width};
use systems::rbf0_xml::{from_xml, to_xml};

fn rec(index: u8, kind: u8) -> Vec<u8> {
    vec![index, kind]
}

fn named(index: u8, kind: u8, name: &str) -> Vec<u8> {
    let mut v = rec(index, kind);
    v.extend((name.len() as u16).to_le_bytes());
    v.extend(name.as_bytes());
    v
}

fn narrow_sample() -> Vec<u8> {
    let mut b = b"RBF0".to_vec();
    b.extend(named(0, 0x00, "Root"));
    b.extend([0, 0, 0, 0, 1, 0]);
    b.extend(named(1, 0x60, "key"));
    b.extend([1, 0, b'v']);
    b.extend(named(2, 0x00, "Child"));
    b.extend([0, 0, 0, 0, 0, 0]);
    b.extend([0xFD, 0xFF, 3, 0, 0, 0, b'h', b'i', 0]);
    b.extend([0xFF, 0xFF]);
    b.extend(named(3, 0x10, "n"));
    b.extend(7u32.to_le_bytes());
    b.extend([0xFF, 0xFF]);
    b
}

#[test]
fn parses_a_hand_built_narrow_stream() {
    let bytes = narrow_sample();
    let doc = rbf0::parse(&bytes).unwrap();
    assert_eq!(doc.width, Width::Narrow);
    assert_eq!(doc.root.name, "Root");
    assert_eq!(
        doc.root.attrs,
        vec![Value {
            name: "key".into(),
            data: ValueData::Str("v".into())
        }]
    );
    assert_eq!(doc.root.content.len(), 2);
    assert_eq!(rbf0::encode(&doc).unwrap(), bytes);
}

#[test]
fn writes_the_documented_xml_form() {
    let doc = rbf0::parse(&narrow_sample()).unwrap();
    let xml = to_xml(&doc);
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rbf0 width=\"narrow\">\n  <Root key=\"v\">\n    <Child>hi</Child>\n    \
         <n x-int=\"7\"/>\n  </Root>\n</rbf0>\n"
    );
    assert_eq!(from_xml(&xml).unwrap(), doc);
}

#[test]
fn wide_stream_starts_with_the_prologue() {
    let doc = Document {
        width: Width::Wide,
        root: Element {
            name: "R".into(),
            attrs: vec![],
            content: vec![Node::Value(Value {
                name: "f".into(),
                data: ValueData::Float(1.5f64.to_bits()),
            })],
        },
    };
    let bytes = rbf0::encode(&doc).unwrap();
    assert_eq!(&bytes[..8], b"RBF0\xFB\xFF\x01\x00");
    assert_eq!(rbf0::parse(&bytes).unwrap(), doc);
    assert!(to_xml(&doc).contains("<f x-float=\"1.5\"/>"));
}

#[test]
fn rejects_what_it_cannot_reproduce() {
    let good = narrow_sample();
    assert_eq!(rbf0::parse(b"XBF0").unwrap_err(), Error::BadMagic);
    assert!(matches!(
        rbf0::parse(&good[..good.len() - 1]),
        Err(Error::Truncated(_))
    ));
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(rbf0::parse(&trailing).unwrap_err(), Error::TrailingBytes(1));
    let mut words = good.clone();
    words[4 + 2 + 2 + 4] = 1;
    assert!(matches!(rbf0::parse(&words), Err(Error::HeaderWords(_))));
    let mut bad_index = b"RBF0".to_vec();
    bad_index.extend(rec(5, 0));
    assert!(matches!(
        rbf0::parse(&bad_index),
        Err(Error::BadDescriptor { .. })
    ));
    let mut dup = b"RBF0".to_vec();
    dup.extend(named(0, 0, "A"));
    dup.extend([0, 0, 0, 0, 0, 0]);
    dup.extend(named(1, 0, "A"));
    assert!(matches!(
        rbf0::parse(&dup),
        Err(Error::DuplicateDescriptor(_))
    ));
    let mut short_attrs = b"RBF0".to_vec();
    short_attrs.extend(named(0, 0, "A"));
    short_attrs.extend([0, 0, 0, 0, 1, 0, 0xFF, 0xFF]);
    assert!(matches!(
        rbf0::parse(&short_attrs),
        Err(Error::AttributeCount(_))
    ));
    let mut unknown = b"RBF0".to_vec();
    unknown.extend(named(0, 0, "A"));
    unknown.extend([0; 6]);
    unknown.extend(named(1, 0x77, "x"));
    assert!(matches!(
        rbf0::parse(&unknown),
        Err(Error::UnknownRecord { kind: 0x77, .. })
    ));
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let mut bytes = b"RBF0".to_vec();
    bytes.extend(named(0, 0, "A"));
    bytes.extend([0; 6]);
    for _ in 0..rbf0::MAX_DEPTH + 1 {
        bytes.extend(rec(0, 0));
        bytes.extend([0; 6]);
    }
    assert_eq!(
        rbf0::parse(&bytes).unwrap_err(),
        Error::TooDeep(rbf0::MAX_DEPTH)
    );
}

#[test]
fn awkward_names_and_texts_survive_xml() {
    let value = |name: &str, data| Value {
        name: name.into(),
        data,
    };
    let doc = Document {
        width: Width::Wide,
        root: Element {
            name: "xmlns:xi".into(),
            attrs: vec![value(
                "xmlns:xi",
                ValueData::Str("http://example/a&b\"<>\n\t\r".into()),
            )],
            content: vec![
                Node::Text(b"a < b & c\0".to_vec()),
                Node::Text(b"no terminator".to_vec()),
                Node::Text(vec![0xFF, 0x00, 0xFE]),
                Node::Text(vec![0]),
                Node::Text(vec![]),
                Node::Text(b"ctl \x01 inside\0".to_vec()),
                Node::Value(value("x-odd name", ValueData::Bool(false))),
                Node::Value(value(
                    "v",
                    ValueData::Vec3([
                        f64::NAN.to_bits() | 5,
                        f64::INFINITY.to_bits(),
                        (-0.0f64).to_bits(),
                    ]),
                )),
                Node::Element(Element {
                    name: "name".into(),
                    attrs: vec![
                        value("a", ValueData::Int(u64::MAX)),
                        value("b", ValueData::Float(1e300f64.to_bits())),
                    ],
                    content: vec![Node::Text(b"  \n  \0".to_vec())],
                }),
                Node::Element(Element {
                    name: "x-element".into(),
                    attrs: vec![value("name", ValueData::Str("clash".into()))],
                    content: vec![Node::Text(b"t\0".to_vec())],
                }),
            ],
        },
    };
    let bytes = rbf0::encode(&doc).unwrap();
    let xml = to_xml(&doc);
    let back = from_xml(&xml).unwrap();
    assert_eq!(back, doc);
    assert_eq!(rbf0::encode(&back).unwrap(), bytes);
}

#[test]
fn narrow_values_that_do_not_fit_are_refused() {
    let doc = Document {
        width: Width::Narrow,
        root: Element {
            name: "R".into(),
            attrs: vec![],
            content: vec![Node::Value(Value {
                name: "i".into(),
                data: ValueData::Int(1 << 32),
            })],
        },
    };
    assert_eq!(rbf0::encode(&doc).unwrap_err(), Error::ValueRange(32));
}

#[test]
fn hand_edited_xml_changes_the_binary() {
    let doc = rbf0::parse(&narrow_sample()).unwrap();
    let edited = to_xml(&doc)
        .replace("x-int=\"7\"", "x-int=\"9\"")
        .replace(">hi<", ">yo<");
    let bytes = rbf0::encode(&from_xml(&edited).unwrap()).unwrap();
    let mut expected = narrow_sample();
    let at = expected.len() - 2 - 4;
    expected[at] = 9;
    let text = expected.windows(3).position(|w| w == b"hi\0").unwrap();
    expected[text..text + 2].copy_from_slice(b"yo");
    assert_eq!(bytes, expected);
}

#[test]
fn xml_errors_are_reported() {
    assert!(from_xml("<rbf0 width=\"wide\"></rbf0>").is_err());
    assert!(from_xml("<rbf0 width=\"huge\"><A/></rbf0>").is_err());
    assert!(from_xml("<other/>").is_err());
    assert!(from_xml("<rbf0 width=\"wide\"><A x-int=\"1\" x-bool=\"true\"/></rbf0>").is_err());
    assert!(from_xml("<rbf0 width=\"wide\"><A>text<B/></A></rbf0>").is_err());
}

fn scalar(narrow: bool) -> BoxedStrategy<u64> {
    if narrow {
        any::<u32>().prop_map(u64::from).boxed()
    } else {
        any::<u64>().boxed()
    }
}

fn string() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..6).prop_map(|c| c.into_iter().collect())
}

fn name_pool() -> impl Strategy<Value = Vec<String>> {
    let name = prop_oneof![
        "[A-Za-z_][A-Za-z0-9_.-]{0,6}",
        Just("xmlns:xi".to_string()),
        Just("x-test".to_string()),
        Just("rbf0".to_string()),
        Just("name".to_string()),
        Just(String::new()),
        string(),
    ];
    prop::collection::vec(name, 1..8)
}

fn value(pool: Vec<String>, narrow: bool) -> impl Strategy<Value = Value> {
    let data = prop_oneof![
        scalar(narrow).prop_map(ValueData::Int),
        scalar(narrow).prop_map(ValueData::Float),
        any::<bool>().prop_map(ValueData::Bool),
        (scalar(narrow), scalar(narrow), scalar(narrow))
            .prop_map(|(a, b, c)| ValueData::Vec3([a, b, c])),
        string().prop_map(ValueData::Str),
    ];
    (prop::sample::select(pool), data).prop_map(|(name, data)| Value { name, data })
}

fn text() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..8),
        string().prop_map(|s| {
            let mut b = s.into_bytes();
            b.push(0);
            b
        }),
    ]
}

fn element(pool: Vec<String>, narrow: bool) -> impl Strategy<Value = Element> {
    let attrs = prop::collection::vec(value(pool.clone(), narrow), 0..3).boxed();
    let leaf = (
        prop::sample::select(pool.clone()),
        attrs.clone(),
        prop::collection::vec(text().prop_map(Node::Text), 0..2),
    )
        .prop_map(|(name, attrs, content)| Element {
            name,
            attrs,
            content,
        });
    leaf.prop_recursive(4, 24, 4, move |inner| {
        let node = prop_oneof![
            inner.prop_map(Node::Element),
            value(pool.clone(), narrow).prop_map(Node::Value),
            text().prop_map(Node::Text),
        ];
        (
            prop::sample::select(pool.clone()),
            attrs.clone(),
            prop::collection::vec(node, 0..4),
        )
            .prop_map(|(name, attrs, content)| Element {
                name,
                attrs,
                content,
            })
    })
}

fn document() -> impl Strategy<Value = Document> {
    (name_pool(), any::<bool>()).prop_flat_map(|(pool, narrow)| {
        element(pool, narrow).prop_map(move |root| Document {
            width: if narrow { Width::Narrow } else { Width::Wide },
            root,
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn binary_round_trip(doc in document()) {
        let bytes = rbf0::encode(&doc).unwrap();
        prop_assert_eq!(rbf0::parse(&bytes).unwrap(), doc);
    }

    #[test]
    fn xml_round_trip_gives_identical_bytes(doc in document()) {
        let bytes = rbf0::encode(&doc).unwrap();
        let xml = to_xml(&doc);
        let back = from_xml(&xml).unwrap();
        prop_assert_eq!(&back, &doc);
        prop_assert_eq!(rbf0::encode(&back).unwrap(), bytes);
    }

    #[test]
    fn damaged_streams_never_panic_and_stay_canonical(doc in document(), cut in any::<prop::sample::Index>(), flip in any::<prop::sample::Index>(), bit in 0u8..8) {
        let bytes = rbf0::encode(&doc).unwrap();
        let mut damaged = bytes.clone();
        let at = flip.index(damaged.len());
        damaged[at] ^= 1 << bit;
        for candidate in [damaged.clone(), bytes[..cut.index(bytes.len() + 1)].to_vec()] {
            if let Ok(parsed) = rbf0::parse(&candidate) {
                prop_assert_eq!(rbf0::encode(&parsed).unwrap(), candidate);
            }
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic(tail in prop::collection::vec(any::<u8>(), 0..80)) {
        let mut bytes = b"RBF0".to_vec();
        bytes.extend(tail);
        if let Ok(parsed) = rbf0::parse(&bytes) {
            prop_assert_eq!(rbf0::encode(&parsed).unwrap(), bytes);
        }
    }

    #[test]
    fn xml_parser_never_panics(src in "\\PC{0,120}") {
        let _ = from_xml(&src);
    }
}
