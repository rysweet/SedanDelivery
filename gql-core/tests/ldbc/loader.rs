//! Synthetic LDBC Social Network Benchmark dataset builder.
//!
//! Creates a small but schema-faithful social network graph following the
//! LDBC SNB Interactive v2 schema, suitable for running the 14 complex read
//! queries (IC1–IC14) and the 16 write operations without requiring an
//! external data download.
//!
//! Entity counts: 8 Person, 4 Place, 3 Tag, 2 TagClass, 3 Organisation,
//! 3 Forum, 6 Post, 3 Comment.
//!
//! Relationship counts: 7 KNOWS, 8 IS_LOCATED_IN, 7 IS_PART_OF, 2 WORK_AT,
//! 1 STUDY_AT, 4 HAS_MEMBER, 3 CONTAINER_OF, 6 HAS_CREATOR, 3 REPLY_OF,
//! 4 LIKES, 6 HAS_TAG, 3 HAS_INTEREST, 2 HAS_TYPE.

use indexmap::IndexMap;

use gql_core::storage::{GraphStore, InMemoryGraphStore, NodeId};
use gql_core::types::Value;

pub const GRAPH: &str = "ldbc";

// ── Node ID bag returned from the builder ────────────────────────────────────

pub struct LdbcIds {
    // Persons
    pub alice: NodeId,
    pub bob: NodeId,
    pub charlie: NodeId,
    pub diana: NodeId,
    pub eve: NodeId,
    pub frank: NodeId,
    pub grace: NodeId,
    pub henry: NodeId,

    // Places
    pub berlin: NodeId,
    pub tokyo: NodeId,
    pub munich: NodeId,
    pub paris: NodeId,
    pub germany: NodeId,
    pub japan: NodeId,
    pub france: NodeId,
    pub europe: NodeId,
    pub asia: NodeId,

    // Tags
    pub tag_technology: NodeId,
    pub tag_sports: NodeId,
    pub tag_music: NodeId,
    pub tag_travel: NodeId,

    // TagClasses
    pub tc_topic: NodeId,
    pub tc_interest: NodeId,

    // Organisations
    pub techcorp: NodeId,
    pub unione: NodeId,
    pub asiacorp: NodeId,

    // Forums
    pub tech_forum: NodeId,
    pub sports_forum: NodeId,

    // Posts
    pub post1: NodeId, // by Alice, technology, in TechForum
    pub post2: NodeId, // by Bob,   sports,     in SportsForum
    pub post3: NodeId, // by Charlie, technology, in TechForum
    pub post4: NodeId, // by Eve,   travel,     (no forum)
    pub post5: NodeId, // by Frank, technology, (no forum)
    pub post6: NodeId, // by Bob,   music,      (no forum)

    // Comments
    pub comment1: NodeId, // by Bob,     REPLY_OF post1
    pub comment2: NodeId, // by Charlie, REPLY_OF post1, HAS_TAG technology
    pub comment3: NodeId, // by Diana,   REPLY_OF post2
}

// ── Builder ──────────────────────────────────────────────────────────────────

fn node(store: &InMemoryGraphStore, labels: &[&str], props: &[(&str, Value)]) -> NodeId {
    let labels: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
    let properties: IndexMap<String, Value> = props
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    store.create_node(GRAPH, labels, properties).unwrap()
}

fn edge(store: &InMemoryGraphStore, src: NodeId, dst: NodeId, rel: &str, props: &[(&str, Value)]) {
    let properties: IndexMap<String, Value> = props
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    store
        .create_edge(GRAPH, src, dst, rel.to_string(), properties)
        .unwrap();
}

/// Build the full synthetic LDBC graph and return the store + all node IDs.
pub fn build() -> (InMemoryGraphStore, LdbcIds) {
    let store = InMemoryGraphStore::default();
    store.create_graph(GRAPH).unwrap();

    // ── Persons ───────────────────────────────────────────────────────────────

    let alice = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Alice".into())),
            ("lastName", Value::Str("Smith".into())),
            ("birthday", Value::Int(19_900_115)),
            ("creationDate", Value::Int(20_120_101)),
            ("gender", Value::Str("female".into())),
        ],
    );
    let bob = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Bob".into())),
            ("lastName", Value::Str("Jones".into())),
            ("birthday", Value::Int(19_880_520)),
            ("creationDate", Value::Int(20_120_201)),
            ("gender", Value::Str("male".into())),
        ],
    );
    let charlie = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Charlie".into())),
            ("lastName", Value::Str("Brown".into())),
            ("birthday", Value::Int(19_920_310)),
            ("creationDate", Value::Int(20_120_301)),
            ("gender", Value::Str("male".into())),
        ],
    );
    let diana = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Diana".into())),
            ("lastName", Value::Str("Prince".into())),
            ("birthday", Value::Int(19_850_715)),
            ("creationDate", Value::Int(20_130_101)),
            ("gender", Value::Str("female".into())),
        ],
    );
    let eve = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Eve".into())),
            ("lastName", Value::Str("White".into())),
            ("birthday", Value::Int(19_950_601)),
            ("creationDate", Value::Int(20_140_101)),
            ("gender", Value::Str("female".into())),
        ],
    );
    let frank = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Frank".into())),
            ("lastName", Value::Str("Miller".into())),
            ("birthday", Value::Int(19_830_920)),
            ("creationDate", Value::Int(20_110_601)),
            ("gender", Value::Str("male".into())),
        ],
    );
    let grace = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Grace".into())),
            ("lastName", Value::Str("Lee".into())),
            ("birthday", Value::Int(19_910_303)),
            ("creationDate", Value::Int(20_150_101)),
            ("gender", Value::Str("female".into())),
        ],
    );
    let henry = node(
        &store,
        &["Person"],
        &[
            ("firstName", Value::Str("Henry".into())),
            ("lastName", Value::Str("Wilson".into())),
            ("birthday", Value::Int(19_870_811)),
            ("creationDate", Value::Int(20_160_101)),
            ("gender", Value::Str("male".into())),
        ],
    );

    // ── Places ────────────────────────────────────────────────────────────────

    let europe = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Europe".into())),
            ("type", Value::Str("Continent".into())),
        ],
    );
    let asia = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Asia".into())),
            ("type", Value::Str("Continent".into())),
        ],
    );

    let germany = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Germany".into())),
            ("type", Value::Str("Country".into())),
        ],
    );
    let japan = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Japan".into())),
            ("type", Value::Str("Country".into())),
        ],
    );
    let france = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("France".into())),
            ("type", Value::Str("Country".into())),
        ],
    );

    let berlin = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Berlin".into())),
            ("type", Value::Str("City".into())),
        ],
    );
    let tokyo = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Tokyo".into())),
            ("type", Value::Str("City".into())),
        ],
    );
    let munich = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Munich".into())),
            ("type", Value::Str("City".into())),
        ],
    );
    let paris = node(
        &store,
        &["Place"],
        &[
            ("name", Value::Str("Paris".into())),
            ("type", Value::Str("City".into())),
        ],
    );

    // city → country → continent
    edge(&store, berlin, germany, "IS_PART_OF", &[]);
    edge(&store, tokyo, japan, "IS_PART_OF", &[]);
    edge(&store, munich, germany, "IS_PART_OF", &[]);
    edge(&store, paris, france, "IS_PART_OF", &[]);
    edge(&store, germany, europe, "IS_PART_OF", &[]);
    edge(&store, japan, asia, "IS_PART_OF", &[]);
    edge(&store, france, europe, "IS_PART_OF", &[]);

    // person → city
    edge(&store, alice, berlin, "IS_LOCATED_IN", &[]);
    edge(&store, bob, berlin, "IS_LOCATED_IN", &[]);
    edge(&store, charlie, tokyo, "IS_LOCATED_IN", &[]);
    edge(&store, diana, munich, "IS_LOCATED_IN", &[]);
    edge(&store, eve, paris, "IS_LOCATED_IN", &[]);
    edge(&store, frank, berlin, "IS_LOCATED_IN", &[]);
    edge(&store, grace, tokyo, "IS_LOCATED_IN", &[]);
    edge(&store, henry, paris, "IS_LOCATED_IN", &[]);

    // ── Tags & TagClasses ─────────────────────────────────────────────────────

    let tc_topic = node(
        &store,
        &["TagClass"],
        &[("name", Value::Str("TopicClass".into()))],
    );
    let tc_interest = node(
        &store,
        &["TagClass"],
        &[("name", Value::Str("InterestClass".into()))],
    );

    let tag_technology = node(
        &store,
        &["Tag"],
        &[("name", Value::Str("technology".into()))],
    );
    let tag_sports = node(&store, &["Tag"], &[("name", Value::Str("sports".into()))]);
    let tag_music = node(&store, &["Tag"], &[("name", Value::Str("music".into()))]);
    let tag_travel = node(&store, &["Tag"], &[("name", Value::Str("travel".into()))]);

    edge(&store, tag_technology, tc_topic, "HAS_TYPE", &[]);
    edge(&store, tag_sports, tc_interest, "HAS_TYPE", &[]);
    edge(&store, tag_music, tc_topic, "HAS_TYPE", &[]);

    // Person interests
    edge(&store, alice, tag_technology, "HAS_INTEREST", &[]);
    edge(&store, bob, tag_sports, "HAS_INTEREST", &[]);
    edge(&store, charlie, tag_music, "HAS_INTEREST", &[]);

    // ── Organisations ─────────────────────────────────────────────────────────

    let techcorp = node(
        &store,
        &["Organisation"],
        &[
            ("name", Value::Str("TechCorp".into())),
            ("type", Value::Str("company".into())),
        ],
    );
    let unione = node(
        &store,
        &["Organisation"],
        &[
            ("name", Value::Str("UniOne".into())),
            ("type", Value::Str("university".into())),
        ],
    );
    let asiacorp = node(
        &store,
        &["Organisation"],
        &[
            ("name", Value::Str("AsiaCorp".into())),
            ("type", Value::Str("company".into())),
        ],
    );

    edge(&store, techcorp, germany, "IS_LOCATED_IN", &[]);
    edge(&store, unione, germany, "IS_LOCATED_IN", &[]);
    edge(&store, asiacorp, japan, "IS_LOCATED_IN", &[]);

    edge(
        &store,
        alice,
        techcorp,
        "WORK_AT",
        &[("workFrom", Value::Int(2015))],
    );
    edge(
        &store,
        bob,
        techcorp,
        "WORK_AT",
        &[("workFrom", Value::Int(2016))],
    );
    edge(
        &store,
        charlie,
        unione,
        "STUDY_AT",
        &[("classYear", Value::Int(2012))],
    );

    // ── Social Graph (KNOWS) ──────────────────────────────────────────────────
    // Alice --knows--> Bob --knows--> Diana
    //      \--knows--> Charlie --knows--> Frank
    //                           Bob --knows--> Eve --knows--> Henry
    //                                         Diana --knows--> Grace

    edge(
        &store,
        alice,
        bob,
        "KNOWS",
        &[("creationDate", Value::Int(20_130_101))],
    );
    edge(
        &store,
        alice,
        charlie,
        "KNOWS",
        &[("creationDate", Value::Int(20_130_201))],
    );
    edge(
        &store,
        bob,
        diana,
        "KNOWS",
        &[("creationDate", Value::Int(20_140_101))],
    );
    edge(
        &store,
        bob,
        eve,
        "KNOWS",
        &[("creationDate", Value::Int(20_140_201))],
    );
    edge(
        &store,
        charlie,
        frank,
        "KNOWS",
        &[("creationDate", Value::Int(20_140_301))],
    );
    edge(
        &store,
        diana,
        grace,
        "KNOWS",
        &[("creationDate", Value::Int(20_150_101))],
    );
    edge(
        &store,
        eve,
        henry,
        "KNOWS",
        &[("creationDate", Value::Int(20_150_201))],
    );

    // ── Forums ────────────────────────────────────────────────────────────────

    let tech_forum = node(
        &store,
        &["Forum"],
        &[
            ("title", Value::Str("TechForum".into())),
            ("creationDate", Value::Int(20_110_101)),
        ],
    );
    let sports_forum = node(
        &store,
        &["Forum"],
        &[
            ("title", Value::Str("SportsForum".into())),
            ("creationDate", Value::Int(20_110_201)),
        ],
    );

    edge(
        &store,
        tech_forum,
        alice,
        "HAS_MEMBER",
        &[("creationDate", Value::Int(20_110_115))],
    );
    edge(
        &store,
        tech_forum,
        bob,
        "HAS_MEMBER",
        &[("creationDate", Value::Int(20_110_115))],
    );
    edge(
        &store,
        sports_forum,
        bob,
        "HAS_MEMBER",
        &[("creationDate", Value::Int(20_110_215))],
    );
    edge(
        &store,
        sports_forum,
        charlie,
        "HAS_MEMBER",
        &[("creationDate", Value::Int(20_110_215))],
    );

    // ── Posts ─────────────────────────────────────────────────────────────────

    let post1 = node(
        &store,
        &["Post"],
        &[
            ("content", Value::Str("Tech news today".into())),
            ("creationDate", Value::Int(20_220_101)),
            ("length", Value::Int(100)),
        ],
    );
    let post2 = node(
        &store,
        &["Post"],
        &[
            ("content", Value::Str("Sports update".into())),
            ("creationDate", Value::Int(20_220_201)),
            ("length", Value::Int(80)),
        ],
    );
    let post3 = node(
        &store,
        &["Post"],
        &[
            ("content", Value::Str("New tech discoveries".into())),
            ("creationDate", Value::Int(20_220_301)),
            ("length", Value::Int(120)),
        ],
    );
    let post4 = node(
        &store,
        &["Post"],
        &[
            ("content", Value::Str("Travel to Paris".into())),
            ("creationDate", Value::Int(20_220_401)),
            ("length", Value::Int(90)),
        ],
    );
    let post5 = node(
        &store,
        &["Post"],
        &[
            ("content", Value::Str("Tech startup tips".into())),
            ("creationDate", Value::Int(20_220_501)),
            ("length", Value::Int(110)),
        ],
    );
    let post6 = node(
        &store,
        &["Post"],
        &[
            ("content", Value::Str("Music festival recap".into())),
            ("creationDate", Value::Int(20_220_601)),
            ("length", Value::Int(70)),
        ],
    );

    // HAS_CREATOR: post → person
    edge(&store, post1, alice, "HAS_CREATOR", &[]);
    edge(&store, post2, bob, "HAS_CREATOR", &[]);
    edge(&store, post3, charlie, "HAS_CREATOR", &[]);
    edge(&store, post4, eve, "HAS_CREATOR", &[]);
    edge(&store, post5, frank, "HAS_CREATOR", &[]);
    edge(&store, post6, bob, "HAS_CREATOR", &[]);

    // CONTAINER_OF: forum → post
    edge(&store, tech_forum, post1, "CONTAINER_OF", &[]);
    edge(&store, tech_forum, post3, "CONTAINER_OF", &[]);
    edge(&store, sports_forum, post2, "CONTAINER_OF", &[]);

    // HAS_TAG: post → tag
    edge(&store, post1, tag_technology, "HAS_TAG", &[]);
    edge(&store, post2, tag_sports, "HAS_TAG", &[]);
    edge(&store, post3, tag_technology, "HAS_TAG", &[]);
    edge(&store, post4, tag_travel, "HAS_TAG", &[]);
    edge(&store, post5, tag_technology, "HAS_TAG", &[]);
    edge(&store, post6, tag_music, "HAS_TAG", &[]);

    // LIKES: person → post
    edge(
        &store,
        bob,
        post1,
        "LIKES",
        &[("creationDate", Value::Int(20_220_110))],
    );
    edge(
        &store,
        charlie,
        post1,
        "LIKES",
        &[("creationDate", Value::Int(20_220_111))],
    );
    edge(
        &store,
        diana,
        post2,
        "LIKES",
        &[("creationDate", Value::Int(20_220_210))],
    );
    edge(
        &store,
        alice,
        post2,
        "LIKES",
        &[("creationDate", Value::Int(20_220_211))],
    );

    // ── Comments ──────────────────────────────────────────────────────────────

    let comment1 = node(
        &store,
        &["Comment"],
        &[
            ("content", Value::Str("Great article!".into())),
            ("creationDate", Value::Int(20_220_102)),
            ("length", Value::Int(15)),
        ],
    );
    let comment2 = node(
        &store,
        &["Comment"],
        &[
            ("content", Value::Str("Totally agree with this".into())),
            ("creationDate", Value::Int(20_220_103)),
            ("length", Value::Int(23)),
        ],
    );
    let comment3 = node(
        &store,
        &["Comment"],
        &[
            ("content", Value::Str("Nice sports coverage".into())),
            ("creationDate", Value::Int(20_220_202)),
            ("length", Value::Int(20)),
        ],
    );

    edge(&store, comment1, bob, "HAS_CREATOR", &[]);
    edge(&store, comment2, charlie, "HAS_CREATOR", &[]);
    edge(&store, comment3, diana, "HAS_CREATOR", &[]);

    edge(&store, comment1, post1, "REPLY_OF", &[]);
    edge(&store, comment2, post1, "REPLY_OF", &[]);
    edge(&store, comment3, post2, "REPLY_OF", &[]);

    // Comment2 is tagged → used by IC12
    edge(&store, comment2, tag_technology, "HAS_TAG", &[]);

    let ids = LdbcIds {
        alice,
        bob,
        charlie,
        diana,
        eve,
        frank,
        grace,
        henry,
        berlin,
        tokyo,
        munich,
        paris,
        germany,
        japan,
        france,
        europe,
        asia,
        tag_technology,
        tag_sports,
        tag_music,
        tag_travel,
        tc_topic,
        tc_interest,
        techcorp,
        unione,
        asiacorp,
        tech_forum,
        sports_forum,
        post1,
        post2,
        post3,
        post4,
        post5,
        post6,
        comment1,
        comment2,
        comment3,
    };

    (store, ids)
}
