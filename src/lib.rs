#![forbid(unsafe_code)]

//! The content route technology — a technology of `xmip-core-route`.
//!
//! A Subscription's filter names properties, and each property is read from
//! one source. This source follows a path into the content:
//! `content:<language>:<expression>` runs the path language `<language>` over
//! the first section's Stream through `xmip-core-path` and reads the scalar it
//! finds — `content:dot:order.total` in the Xmip content selector,
//! `content:jsonpath:$.order.total` in RFC 9535. The scalar reads through
//! `route::routable`, as a context value does: as the text a filter compares,
//! a JSON `null` absent, bytes refused (ADR-0046, amended 2026-09-24). A
//! Message with no section has no content and promotes nothing.
//!
//! **Once.** The languages are the ones the [`PathEngine`] this source is
//! configured with carries; nothing here names any of them. Each property's
//! path is compiled through that engine once, when the filter is, and a
//! language the engine does not carry, or an expression it refuses, is
//! refused then. The content is parsed once per Message into each form its
//! paths read, however many properties read it (`path::Content`), so
//! `content:dot:a` and `content:jsonpath:$.b` share one JSON parse. Content
//! the language cannot parse, or a path that lands on an object or an array
//! rather than a value, is an error with the engine's own reason.
//!
//! A route technology does not decide anything: it reads.

use message::Message;
use path::{CompiledPath, Content, Path, PathEngine};
use route::{Reading, Source};

/// The manifest leaf and the prefix a property carries.
pub const TECHNOLOGY: &str = "content";

/// Reads `content:<language>:<expression>` from the first section's content,
/// in the languages its engine carries.
pub struct ContentSource {
    engine: PathEngine,
}

impl ContentSource {
    /// A source reading the languages `engine` carries.
    #[must_use]
    pub const fn new(engine: PathEngine) -> Self {
        Self { engine }
    }
}

impl Source for ContentSource {
    fn technology(&self) -> &'static str {
        TECHNOLOGY
    }

    fn compile(&self, name: &str) -> Result<Box<dyn Reading>, String> {
        let Some((language, expression)) = name.split_once(':') else {
            return Err("a property is content:<language>:<expression>".to_string());
        };
        if expression.is_empty() {
            return Err("an expression is needed after the language".to_string());
        }
        let path = self
            .engine
            .compile(&Path::new(language, expression))
            .map_err(|refused| refused.message)?;
        Ok(Box::new(Compiled(path)))
    }
}

/// A property's path, compiled.
struct Compiled(CompiledPath);

impl Reading for Compiled {
    fn read(&self, _: &Message, content: Option<&Content<'_>>) -> Result<Option<String>, String> {
        let Some(content) = content else {
            return Ok(None);
        };
        let found = self.0.read(content).map_err(|refused| refused.message)?;
        route::routable(&self.0.path().expression, found.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use context::MessageContext;
    use contract::ContractError;
    use message::{MessageSection, MessageTreatment};
    use path::{CompiledExpression, Form, PathLanguage, Rewriting};
    use path_dot::DotLanguage;
    use path_jsonpath::JsonPathLanguage;
    use route::{Gathering, Promoted, SourceError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use stream::Stream;
    use xcore::{MessageId, ScalarValue, SectionId, StreamId};

    const ORDER: &[u8] = br#"{"order": {"id": "A-1", "total": 1500, "weight": 2.5,
        "paid": false, "note": null, "lines": [{"sku": "X"}, {"sku": "Y"}]}}"#;

    fn message(bytes: &[u8]) -> Message {
        let section = MessageSection {
            section_id: SectionId::new(10),
            name: None,
            stream: Stream::new(
                StreamId::new(10),
                bytes.to_vec(),
                Some("application/json".into()),
            ),
            contract: None,
        };
        Message::received(
            MessageId::new(1),
            vec![section],
            MessageContext::new(),
            MessageTreatment::default(),
        )
    }

    fn json() -> ContentSource {
        ContentSource::new(PathEngine::new(vec![
            Box::new(DotLanguage),
            Box::new(JsonPathLanguage),
        ]))
    }

    fn promote(message: &Message, properties: &[&str]) -> Result<Promoted, SourceError> {
        Gathering::new(&[&json()], properties).promote(message)
    }

    fn read_from(message: &Message, name: &str) -> Result<Option<String>, SourceError> {
        let property = format!("content:{name}");
        Ok(promote(message, &[property.as_str()])?
            .get(&property)
            .map(str::to_string))
    }

    fn read(name: &str) -> Result<Option<String>, SourceError> {
        read_from(&message(ORDER), name)
    }

    #[test]
    fn a_dot_selector_and_a_jsonpath_query_read_the_same_scalar_as_text() {
        assert_eq!(read("dot:order.id").expect("text"), Some("A-1".into()));
        assert_eq!(
            read("dot:order.total").expect("integer"),
            Some("1500".into())
        );
        assert_eq!(
            read("dot:order.weight").expect("decimal"),
            Some("2.5".into())
        );
        assert_eq!(
            read("dot:order.paid").expect("boolean"),
            Some("false".into())
        );
        assert_eq!(
            read("dot:order.lines[1].sku").expect("ordinal"),
            Some("Y".into())
        );

        assert_eq!(
            read("jsonpath:$.order.id").expect("text"),
            Some("A-1".into())
        );
        assert_eq!(
            read("jsonpath:$.order.lines[?@.sku == 'Y'].sku").expect("filter"),
            Some("Y".into())
        );
    }

    #[test]
    fn a_null_an_absent_place_and_a_message_without_content_promote_nothing() {
        assert_eq!(read("dot:order.note").expect("null"), None);
        assert_eq!(read("dot:order.missing").expect("absent"), None);
        assert_eq!(read("jsonpath:$.order.missing").expect("absent"), None);

        let empty = Message::received(
            MessageId::new(2),
            Vec::new(),
            MessageContext::new(),
            MessageTreatment::default(),
        );
        assert_eq!(read_from(&empty, "dot:order.id").expect("no content"), None);
    }

    #[test]
    fn an_unloaded_language_a_bad_property_and_content_that_is_not_a_value_are_refused() {
        let language = read("xpath:/order/id").expect_err("no xpath");
        assert_eq!(language.technology, "content");
        assert!(
            language.reason.contains("the languages are dot, jsonpath"),
            "{}",
            language.reason
        );

        let shape = read("order.id").expect_err("no language");
        assert!(shape.reason.contains("content:<language>:<expression>"));
        assert!(read("dot:").is_err());
        assert!(read("dot:lines[").is_err(), "refused as it compiles");

        let not_scalar = read("dot:order.lines").expect_err("an array");
        assert!(not_scalar.reason.contains("not a scalar"));

        let broken = read_from(&message(b"{not json"), "dot:order.id").expect_err("not JSON");
        assert!(broken.reason.contains("not valid JSON"));
    }

    /// A language that reads the Stream's length through a form counting
    /// how often it is parsed.
    struct Measured;

    struct Length(usize);

    static PARSED: AtomicUsize = AtomicUsize::new(0);

    impl Form for Length {
        fn parse(stream: &Stream) -> Result<Self, ContractError> {
            PARSED.fetch_add(1, Ordering::Relaxed);
            Ok(Self(stream.len()))
        }
    }

    impl PathLanguage for Measured {
        fn language(&self) -> &'static str {
            "length"
        }

        fn compile(&self, _: &str) -> Result<Box<dyn CompiledExpression>, ContractError> {
            Ok(Box::new(Self))
        }
    }

    impl CompiledExpression for Measured {
        fn read(&self, content: &Content<'_>) -> Result<Option<ScalarValue>, ContractError> {
            let length = i64::try_from(content.form::<Length>()?.0).unwrap_or(i64::MAX);
            Ok(Some(ScalarValue::Integer(length)))
        }

        fn write(&self, _: &mut Rewriting, _: ScalarValue) -> Result<(), ContractError> {
            Err(ContractError::new("read-only"))
        }
    }

    #[test]
    fn content_is_parsed_once_per_message_however_many_properties_read_it() {
        let source = ContentSource::new(PathEngine::new(vec![Box::new(Measured)]));
        let gathering = Gathering::new(
            &[&source],
            &["content:length:a", "content:length:b", "content:length:c"],
        );
        let order = message(ORDER);
        for _ in 0..100 {
            let promoted = gathering.promote(&order).expect("readable");
            assert_eq!(promoted.len(), 3);
        }
        assert_eq!(PARSED.load(Ordering::Relaxed), 100, "once per Message");
    }

    #[test]
    fn the_technology_is_content_and_the_gathering_reads_the_prefixed_property() {
        assert_eq!(json().technology(), "content");

        let promoted = promote(
            &message(ORDER),
            &["content:dot:order.total", "content:jsonpath:$.order.note"],
        )
        .expect("readable");

        assert_eq!(promoted.get("content:dot:order.total"), Some("1500"));
        assert_eq!(promoted.get("content:jsonpath:$.order.note"), None);
        assert!(
            path::expression::Expression::parse("\"content:dot:order.total\" > 1000")
                .expect("compiles")
                .evaluate(&promoted)
                .holds()
        );
    }
}
