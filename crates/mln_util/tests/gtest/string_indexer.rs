// Ports test/util/string_indexer.test.cpp.

use mln_util::string_indexer::{StringIndexer, string_indexer};

// gtest: StringIndexer.SingletonStringIndexer
#[test]
fn singleton_string_indexer() {
    // EXPECT_GE(stringIndexer().size(), 0): the singleton exists.
    let _ = string_indexer().size();
}

// gtest: StringIndexer.AddStrings
#[test]
fn add_strings() {
    let str_indexer = StringIndexer::new();
    assert_eq!(str_indexer.size(), 0);

    let id1 = str_indexer.get("test string1");
    assert_eq!(id1, 0);
    assert_eq!(str_indexer.size(), 1);

    let s2: &str = "test string2";
    let id2 = str_indexer.get(s2);
    assert_eq!(id2, 1);
    assert_eq!(str_indexer.size(), 2);
}

// gtest: StringIndexer.GetString
#[test]
fn get_string() {
    let str_indexer = StringIndexer::new();
    assert_eq!(str_indexer.size(), 0);

    let s = "test string1";
    let id1 = str_indexer.get(s);
    assert_eq!(id1, 0);
    assert_eq!(str_indexer.size(), 1);

    let str1 = str_indexer.get_string(id1);
    assert_eq!(s, str1);

    let str2 = s;
    let id2 = str_indexer.get(str2);
    assert_eq!(id1, id2);
}

// gtest: StringIndexer.Reallocate
#[test]
fn reallocate() {
    const N: usize = 1000;
    let str_indexer = StringIndexer::new();
    assert_eq!(str_indexer.size(), 0);

    let s = "reallocate test string1";
    let id1 = str_indexer.get(s);
    assert_eq!(id1, 0);
    assert_eq!(str_indexer.size(), 1);

    // force reallocation. assume capacity: 100 strings, 100 * 32 bytes buffer space
    let string_for_i = |i: usize| format!("{0}{0}{0}---{i}", "1234567890");
    for i in 1..N {
        str_indexer.get(&string_for_i(i));
    }
    assert_eq!(str_indexer.size(), N);

    let str1 = str_indexer.get_string(id1);
    assert_eq!(s, str1);

    for i in 1..N {
        let str_n1 = str_indexer.get_string(i);
        assert_eq!(str_n1, string_for_i(i));
    }
}

// gtest: StringIndexer.GetOOBIdentity
#[test]
fn get_oob_identity() {
    let str_indexer = StringIndexer::new();
    assert_eq!(str_indexer.size(), 0);

    let mut s = String::new();
    // #ifndef NDEBUG: EXPECT_DEATH_IF_SUPPORTED(str = strIndexer.get(42),
    // "id < identityToString.size\\(\\)"); a release build returns "".
    for id in [42, usize::MAX /* -1 */] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| str_indexer.get_string(id)));
        if cfg!(debug_assertions) {
            let message = result.expect_err("assertion");
            let message = message
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| message.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            assert!(message.contains("id < identityToString.size()"), "{message}");
        } else {
            s = result.unwrap();
        }
        assert_eq!(s, "");
    }
}
