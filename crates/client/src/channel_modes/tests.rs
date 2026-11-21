use std::collections::BTreeMap;

use super::*;

fn isupport(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn solanum() -> Extbans {
    Extbans {
        prefix: "$".into(),
        types: "acjorsxz".into(),
    }
}

fn ergo() -> Extbans {
    Extbans {
        prefix: String::new(),
        types: "m".into(),
    }
}

fn alice() -> MaskPerson {
    MaskPerson {
        nick: "alice".into(),
        username: Some("~al".into()),
        host: Some("203.0.113.7".into()),
        account: Some("Alice".into()),
    }
}

#[test]
fn flags_put_described_letters_first() {
    let modes = Chanmodes::parse(&isupport(&[("CHANMODES", "beIq,k,flj,CEXnstimz")]));
    assert_eq!(
        sorted_flags(&modes),
        ['C', 'i', 'm', 'n', 's', 't', 'z', 'E', 'X']
    );
    assert_eq!(value_modes(&modes), ['k', 'f', 'l', 'j']);
    assert!(known('n') && !varies('n'));
    assert!(known('j') && varies('j'));
    assert!(!known('X'));
    assert!(described_list('I') && !described_list('Z'));
}

#[test]
fn ranks_and_topic_limits() {
    assert_eq!(Rank::of(Some('~')), Rank::Owner);
    assert_eq!(Rank::of(Some('%')), Rank::HalfOperator);
    assert_eq!(Rank::of(None), Rank::Member);
    assert_eq!(topic_limit(&isupport(&[])), 390);
    assert_eq!(topic_limit(&isupport(&[("TOPICLEN", "307")])), 307);
    assert_eq!(topic_limit(&isupport(&[("TOPICLEN", "x")])), 390);
}

#[test]
fn lines_settings_send() {
    let modes = Chanmodes::parse(&isupport(&[]));
    assert_eq!(topic_line("#a", " **hi** "), "TOPIC #a :\x02hi\x02");
    assert_eq!(toggle_line("#a", 'm', true), "MODE #a +m");
    assert_eq!(toggle_line("#a", 'm', false), "MODE #a -m");
    assert_eq!(
        set_value_line("#a", 'l', " 20 ").as_deref(),
        Some("MODE #a +l 20")
    );
    assert_eq!(set_value_line("#a", 'l', "  "), None);
    assert_eq!(set_value_line("#a", 'k', "two words"), None);
    // Removing the key sends the key when known; the limit takes nothing.
    assert_eq!(
        clear_value_line("#a", 'k', &modes, Some("s3")),
        "MODE #a -k s3"
    );
    assert_eq!(clear_value_line("#a", 'k', &modes, None), "MODE #a -k *");
    assert_eq!(
        clear_value_line("#a", 'l', &modes, Some("20")),
        "MODE #a -l"
    );
    assert_eq!(list_line("#a", 'b', "x!*@*", false), "MODE #a -b x!*@*");
    assert_eq!(status_line("#a", 'o', "bob", true), "MODE #a +o bob");
    assert_eq!(kick_line("#a", "bob", " "), "KICK #a bob");
    assert_eq!(kick_line("#a", "bob", "spam"), "KICK #a bob :spam");
    assert_eq!(ban_line("#a", "bob"), "MODE #a +b bob!*@*");
}

#[test]
fn extbans_come_from_isupport() {
    assert_eq!(Extbans::parse(&isupport(&[])), None);
    let x = Extbans::parse(&isupport(&[("EXTBAN", "$,acjorsxz")])).unwrap();
    assert_eq!(x, solanum());
    assert_eq!(x.account().as_deref(), Some("$a:"));
    assert_eq!(x.mute(), None);
    // Ergo advertises its types without a prefix.
    let x = Extbans::parse(&isupport(&[("EXTBAN", "m")])).unwrap();
    assert_eq!(x, ergo());
    assert_eq!(x.account(), None);
    assert_eq!(x.mute(), Some("m:"));
    let unreal = Extbans::parse(&isupport(&[("EXTBAN", "~,acjnqrtCGOST")])).unwrap();
    assert_eq!(unreal.account().as_deref(), Some("~a:"));
}

#[test]
fn extended_masks_split_into_type_and_argument() {
    let x = solanum();
    assert_eq!(
        extban_of("$a:erin", Some(&x)),
        Some(Extban {
            kind: 'a',
            arg: "erin".into(),
            negated: false
        })
    );
    assert_eq!(
        extban_of("$~a", Some(&x)),
        Some(Extban {
            kind: 'a',
            arg: String::new(),
            negated: true
        })
    );
    assert_eq!(extban_of("$q:x", Some(&x)), None);
    assert_eq!(extban_of("erin", Some(&x)), None);
    // Prefixless extbans need the colon.
    assert_eq!(extban_of("malice", Some(&ergo())), None);
    assert_eq!(extban_of("m:alice", Some(&ergo())).unwrap().arg, "alice");
    assert_eq!(extban_of("$a:erin", None), None);
}

#[test]
fn masks_expand_as_ergo_stores_them() {
    assert_eq!(expand_mask("alice"), "alice!*@*");
    assert_eq!(expand_mask("bob@host"), "*!bob@host");
    assert_eq!(expand_mask("carol!id"), "carol!id@*");
    assert_eq!(expand_mask("*!*@*.example.net"), "*!*@*.example.net");
    assert_eq!(expand_mask("@h"), "*!*@h");
    // An extended mask the network does not advertise is read as a nick.
    assert_eq!(to_mask("$a:erin", Some(&ergo())), "$a:erin!*@*");
    assert_eq!(to_mask("$a:erin", None), "$a:erin");
    assert_eq!(to_mask("$a:erin", Some(&solanum())), "$a:erin");
    assert_eq!(to_mask("m:alice", Some(&ergo())), "m:alice!*@*");
    assert_eq!(to_mask("$z", Some(&solanum())), "$z");
    assert_eq!(to_mask("$q:what", Some(&solanum())), "$q:what");
    assert_eq!(
        to_mask("bad$#overflow", Some(&solanum())),
        "bad!*@*$#overflow"
    );
    assert_eq!(to_mask(" alice ", None), "alice!*@*");
}

#[test]
fn wildcards_match_case_insensitively() {
    assert!(mask_matches("alice!*@*", "Alice!~al@host"));
    assert!(mask_matches("*!*@*.example.net", "x!y@a.b.example.net"));
    assert!(!mask_matches("*!*@*.example.net", "x!y@example.net"));
    assert!(mask_matches("a?ice!*@*", "alice!u@h"));
    assert!(!mask_matches("a?ice!*@*", "aice!u@h"));
    assert!(mask_matches("*", ""));
    assert!(mask_matches("a*b*c", "aXXbYYc"));
    assert!(!mask_matches("a*b*c", "aXXbYY"));
    // Regex characters are plain text.
    assert!(mask_matches("a.b(c)", "a.b(c)"));
    assert!(!mask_matches("a.b", "axb"));
}

#[test]
fn broad_masks_are_named() {
    assert_eq!(broadness("*!*@*"), Some(Broad::Everyone));
    assert_eq!(broadness("*!*@*.com"), Some(Broad::Host));
    assert_eq!(broadness("*!*@203.*"), Some(Broad::Host));
    assert_eq!(broadness("*!*@203.0.*"), Some(Broad::Host));
    assert_eq!(broadness("*!*@*.co.uk"), Some(Broad::Host));
    assert_eq!(broadness("*!*@*.example.net"), None);
    assert_eq!(broadness("*!*@203.0.113.*"), None);
    assert_eq!(broadness("a*!*@*"), Some(Broad::Nick));
    assert_eq!(broadness("al*!*@*"), Some(Broad::Nick));
    assert_eq!(broadness("ali*!*@*"), None);
    // A pinned username is never broad.
    assert_eq!(broadness("*!*alice@*"), None);
    assert_eq!(broadness("alice!*@*"), None);
}

#[test]
fn previews_explain_what_is_added() {
    let p = preview_mask("alice", None);
    assert_eq!(p.mask, "alice!*@*");
    assert!(p.expanded);
    assert_eq!(
        p.scope,
        Some(Scope::Matches(vec![Part {
            kind: PartKind::Nick,
            value: "alice".into(),
            pattern: false
        }]))
    );
    assert!(p.warnings.is_empty());

    let p = preview_mask("*!*@*.example.net", None);
    assert!(!p.expanded);
    assert_eq!(
        p.scope,
        Some(Scope::Matches(vec![Part {
            kind: PartKind::Host,
            value: "*.example.net".into(),
            pattern: true
        }]))
    );

    let p = preview_mask("*", None);
    assert_eq!(p.mask, "*!*@*");
    assert_eq!(p.scope, Some(Scope::Matches(Vec::new())));
    assert_eq!(p.warnings, [Warning::Broad(Broad::Everyone)]);

    assert_eq!(preview_mask("", None), MaskPreview::default());
    assert_eq!(preview_mask("a b", None).error, Some(MaskError::Spaces));
    assert_eq!(preview_mask(":x", None).error, Some(MaskError::Prefix));
    assert_eq!(preview_mask("a,b", None).error, Some(MaskError::Prefix));
    assert_eq!(preview_mask("a,b", None).mask, "");
}

#[test]
fn previews_of_extended_masks() {
    let x = solanum();
    let p = preview_mask("$a:erin", Some(&x));
    assert_eq!(p.extban, Some(ExtbanKind::Account));
    assert_eq!(
        p.scope,
        Some(Scope::Account {
            account: "erin".into(),
            negated: false
        })
    );
    let p = preview_mask("$~a", Some(&x));
    assert_eq!(
        p.scope,
        Some(Scope::Account {
            account: String::new(),
            negated: true
        })
    );
    let p = preview_mask("$z", Some(&x));
    assert_eq!(p.extban, Some(ExtbanKind::Other));
    assert_eq!(p.scope, Some(Scope::Extended { negated: false }));
    let p = preview_mask("$q:x", Some(&x));
    assert_eq!(p.warnings, [Warning::UnknownType("$".into())]);
    assert_eq!(p.scope, None);
    let p = preview_mask("bad$#overflow", Some(&x));
    assert_eq!(
        p.scope,
        Some(Scope::MatchesForward(
            vec![Part {
                kind: PartKind::Nick,
                value: "bad".into(),
                pattern: false
            }],
            "#overflow".into()
        ))
    );

    let p = preview_mask("m:a*", Some(&ergo()));
    assert_eq!(p.mask, "m:a*!*@*");
    assert_eq!(p.extban, Some(ExtbanKind::Mute));
    assert_eq!(p.warnings, [Warning::Broad(Broad::Nick)]);
    // Ergo has no `$` masks, so it reads this as a nick.
    let p = preview_mask("$a:erin", Some(&ergo()));
    assert_eq!(p.mask, "$a:erin!*@*");
    assert_eq!(p.warnings, [Warning::NoExtendedType('$')]);
    // Without EXTBAN, `$` text goes as typed, with a warning.
    let p = preview_mask("$a:erin", None);
    assert_eq!(p.mask, "$a:erin");
    assert_eq!(p.warnings, [Warning::UnknownExtban]);
}

#[test]
fn people_here_match_with_what_is_known() {
    assert!(matches_person("alice!*@*", "Alice", None, None));
    assert!(matches_person(
        "*!*@203.*",
        "bob",
        Some("u"),
        Some("203.0.113.1")
    ));
    assert!(!matches_person(
        "*!*@203.*",
        "bob",
        Some("u"),
        Some("198.51.100.1")
    ));
    // Without their address, a mask that looks at it does not count.
    assert!(!matches_person("*!*@203.*", "bob", None, None));
    assert!(matches_person("b*!*@*", "bob", None, Some("h")));
}

#[test]
fn the_builder_reads_parts_back_from_the_mask() {
    let p = alice();
    let account = Some("$a:");
    assert_eq!(
        parts_of("alice!*@*", &p, account),
        MaskParts {
            nick: true,
            ..Default::default()
        }
    );
    assert_eq!(
        parts_of("*!~AL@203.0.113.7", &p, account),
        MaskParts {
            user: true,
            host: true,
            ..Default::default()
        }
    );
    assert_eq!(
        parts_of("$a:alice", &p, account),
        MaskParts {
            account: true,
            ..Default::default()
        }
    );
    // A hand-edited wildcard leaves its box unticked.
    assert_eq!(parts_of("*!*@*.0.113.7", &p, account), MaskParts::default());
    assert_eq!(parts_of("alice", &p, account), MaskParts::default());
}

#[test]
fn the_builder_toggles_parts() {
    let p = alice();
    let account = Some("$a:");
    assert_eq!(default_mask(&p, account), "$a:Alice");
    assert_eq!(default_mask(&p, None), "*!*@203.0.113.7");
    let nameless = MaskPerson {
        nick: "zed".into(),
        ..Default::default()
    };
    assert_eq!(default_mask(&nameless, account), "zed!*@*");
    // An address part drops the account; the account replaces them all.
    let m = toggled_mask("$a:Alice", &p, MaskPart::Nick, true, account);
    assert_eq!(m, "alice!*@*");
    let m = toggled_mask(&m, &p, MaskPart::Host, true, account);
    assert_eq!(m, "alice!*@203.0.113.7");
    let m = toggled_mask(&m, &p, MaskPart::Nick, false, account);
    assert_eq!(m, "*!*@203.0.113.7");
    assert_eq!(
        toggled_mask(&m, &p, MaskPart::Account, true, account),
        "$a:Alice"
    );
    // Unknown parts stay wildcards.
    assert_eq!(
        toggled_mask("zed!*@*", &nameless, MaskPart::Host, true, account),
        "zed!*@*"
    );
    // `*` reports no account.
    let out = MaskPerson {
        account: Some("*".into()),
        ..alice()
    };
    assert_eq!(out.known_account(), None);
    assert_eq!(parts_of("$a:alice", &p, account).count(), 1);
}
