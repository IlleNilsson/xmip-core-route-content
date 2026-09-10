#![forbid(unsafe_code)]

//! The content route technology — a technology of `xmip-core-route`.
//!
//! A Subscription's filter names properties, and each property is read from
//! one source. This source follows a path into the content:
//! `content:<language>:<expression>` runs the path language `<language>` over
//! the first section's Stream through `xmip-core-path` and reads the scalar it
//! finds — `content:dot:order.total` in the Xmip content selector,
//! `content:jsonpath:$.order.total` in RFC 9535. The scalar reads as the text
//! a filter compares; a JSON `null` reads as nothing promoted; a Message with
//! no section has no content and promotes nothing. A language this technology
//! does not carry, content the language cannot parse, or a path that lands on
//! an object or an array rather than a value, is an error with the engine's
//! own reason. ADR-0046.
//!
//! Two languages, because the content route reads JSON today: `dot` through
//! `xmip-core-path-dot` and `jsonpath` through `xmip-core-path-jsonpath`. A
//! third is one more arm in [`ContentSource::read`], not a new shape.
//!
//! A route technology does not decide anything: it reads.

use message::Message;
use path::{Path, PathEngine};
use path_dot::{DotEngine, DotStructure};
use path_jsonpath::{JsonPathEngine, JsonPathStructure};
use route::{Source, SourceError};
use xcore::ScalarValue;

/// The manifest leaf and the prefix a property carries.
pub const TECHNOLOGY: &str = "content";

/// The path languages this technology carries, in the order the crate
/// documentation gives them.
pub const LANGUAGES: [&str; 2] = ["dot", "jsonpath"];

/// Reads `content:<language>:<expression>` from the first section's content.
pub struct ContentSource;

impl Source for ContentSource {
    fn technology(&self) -> &'static str {
        TECHNOLOGY
    }

    fn read(&self, message: &Message, name: &str) -> Result<Option<String>, SourceError> {
        let Some((language, expression)) = name.split_once(':') else {
            return Err(SourceError::new(
                TECHNOLOGY,
                name,
                "a property is content:<language>:<expression>",
            ));
        };
        if expression.is_empty() {
            return Err(SourceError::new(
                TECHNOLOGY,
                name,
                "an expression is needed after the language",
            ));
        }

        let Some(section) = message.sections().first() else {
            return Ok(None);
        };
        let stream = &section.stream;
        let path = Path::new(language, expression);
        let refuse = |reason: String| SourceError::new(TECHNOLOGY, name, reason);

        let found = match language {
            "dot" => {
                let structure = DotStructure::parse(stream).map_err(|e| refuse(e.to_string()))?;
                DotEngine.read(&structure, &path)
            }
            "jsonpath" => {
                let structure =
                    JsonPathStructure::parse(stream).map_err(|e| refuse(e.to_string()))?;
                JsonPathEngine.read(&structure, &path)
            }
            other => {
                return Err(refuse(format!(
                    "{other} is not a path language this technology carries; the languages \
                     are {}",
                    LANGUAGES.join(" and ")
                )));
            }
        }
        .map_err(|e| refuse(e.to_string()))?;

        match found {
            None | Some(ScalarValue::Null) => Ok(None),
            Some(ScalarValue::Binary(bytes)) => Err(refuse(format!(
                "{expression} is {} bytes, and bytes are not routable as text",
                bytes.len()
            ))),
            Some(ScalarValue::Text(text)) => Ok(Some(text)),
            Some(ScalarValue::Bool(flag)) => Ok(Some(flag.to_string())),
            Some(ScalarValue::Integer(number)) => Ok(Some(number.to_string())),
            Some(ScalarValue::Decimal(number)) => Ok(Some(number.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use context::MessageContext;
    use message::{MessageSection, MessageTreatment};
    use route::{Predicate, Value};
    use stream::Stream;
    use xcore::{MessageId, SectionId, StreamId};

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

    fn read(name: &str) -> Result<Option<String>, SourceError> {
        ContentSource.read(&message(ORDER), name)
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
        assert_eq!(
            ContentSource
                .read(&empty, "dot:order.id")
                .expect("no content"),
            None
        );
    }

    #[test]
    fn an_unknown_language_a_bad_property_and_content_that_is_not_a_value_are_refused() {
        let language = read("xpath:/order/id").expect_err("no xpath");
        assert_eq!(language.technology, "content");
        assert!(language.reason.contains("dot and jsonpath"));

        let shape = read("order.id").expect_err("no language");
        assert!(shape.reason.contains("content:<language>:<expression>"));
        assert!(read("dot:").is_err());

        let not_scalar = read("dot:order.lines").expect_err("an array");
        assert!(not_scalar.reason.contains("not a scalar"));

        let broken = ContentSource
            .read(&message(b"{not json"), "dot:order.id")
            .expect_err("not JSON");
        assert!(broken.reason.contains("not valid JSON"));
    }

    #[test]
    fn the_technology_is_content_and_promote_reads_the_prefixed_property() {
        assert_eq!(ContentSource.technology(), "content");

        let sources: [&dyn Source; 1] = [&ContentSource];
        let promoted = route::promote(
            &message(ORDER),
            &sources,
            &["content:dot:order.total", "content:jsonpath:$.order.note"],
        )
        .expect("readable");

        assert_eq!(promoted.get("content:dot:order.total"), Some("1500"));
        assert_eq!(promoted.get("content:jsonpath:$.order.note"), None);
        assert!(
            Predicate::greater_than("content:dot:order.total", Value::Integer(1000))
                .test(&promoted)
                .passed()
        );
    }
}
