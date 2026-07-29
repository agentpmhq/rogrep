//! Tantivy schema. One document per turn. Bumping INDEX_SCHEMA_VERSION
//! moves the index to a new `index/v<N>` directory and triggers a rebuild —
//! never migrate in place.

use tantivy::schema::{
    IndexRecordOption, NumericOptions, Schema, TextFieldIndexing, TextOptions, FAST,
    STORED, STRING,
};

pub const INDEX_SCHEMA_VERSION: u32 = 2;

#[derive(Clone)]
pub struct Fields {
    pub text: tantivy::schema::Field,
    pub doc_key: tantivy::schema::Field,
    pub conversation_id: tantivy::schema::Field,
    pub turn_index: tantivy::schema::Field,
    pub exchange_ordinal: tantivy::schema::Field,
    pub ts: tantivy::schema::Field,
    pub role: tantivy::schema::Field,
    pub visible: tantivy::schema::Field,
    pub turn_facets: tantivy::schema::Field,
    pub cwd: tantivy::schema::Field,
    pub file: tantivy::schema::Field,
    pub project: tantivy::schema::Field,
    pub provider: tantivy::schema::Field,
    pub model: tantivy::schema::Field,
}

pub fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    // Full turn text: positions for phrase queries, STORED so excerpts come
    // straight from the doc store (sub-ms) instead of re-reading source
    // files.
    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    let text = builder.add_text_field("text", text_options);
    // Precise delete-by-term for the tail refresh.
    let doc_key = builder.add_text_field("doc_key", STRING);
    let conversation_id = builder.add_text_field("conversation_id", STRING | STORED | FAST);
    let turn_index = builder.add_u64_field("turn_index", NumericOptions::default().set_indexed().set_stored().set_fast());
    let exchange_ordinal = builder.add_u64_field("exchange_ordinal", NumericOptions::default().set_stored().set_fast());
    let ts = builder.add_i64_field("ts", NumericOptions::default().set_indexed().set_stored().set_fast());
    let role = builder.add_text_field("role", STRING | STORED);
    // "true"/"false": is this turn user-visible content (vs harness-injected
    // context, synthetic attachments, system noise)? Corpus search filters
    // to visible turns; conversation-scoped find greps everything.
    let visible = builder.add_text_field("visible", STRING);
    // Raw facet terms (`tool:bash`, `tool_status:failed`, `git_pr_num:48`).
    let turn_facets = builder.add_text_field("turn_facets", STRING);
    let cwd = builder.add_text_field("cwd", STRING);
    let file = builder.add_text_field("file", STRING);
    let project = builder.add_text_field("project", STRING);
    let provider = builder.add_text_field("provider", STRING);
    let model = builder.add_text_field("model", STRING);
    let schema = builder.build();
    let fields = Fields {
        text,
        doc_key,
        conversation_id,
        turn_index,
        exchange_ordinal,
        ts,
        role,
        visible,
        turn_facets,
        cwd,
        file,
        project,
        provider,
        model,
    };
    (schema, fields)
}
