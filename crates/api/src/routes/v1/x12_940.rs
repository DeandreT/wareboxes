//! Narrow, versioned X12 940 profile mapped into canonical fulfillment demand.

use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use wareboxes_api_contract::v1::{
    FulfillmentOrderDestination, IntegrationOrderEnvelopeLineRequest,
    IntegrationOrderEnvelopeRequest,
};

const MAX_SEGMENTS: usize = 10_000;
const MAX_ELEMENTS_PER_SEGMENT: usize = 64;
const MAX_ELEMENT_BYTES: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ParseError(String);

#[derive(Debug)]
struct Segment {
    tag: String,
    elements: Vec<String>,
}

pub(crate) fn parse(input: &[u8]) -> Result<IntegrationOrderEnvelopeRequest, ParseError> {
    let text = std::str::from_utf8(input).map_err(|_| invalid("document must be valid UTF-8"))?;
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\r' | '\n'))
    {
        return Err(invalid(
            "document contains an unsupported control character",
        ));
    }
    let bytes = text.as_bytes();
    if bytes.len() < 106 || !bytes.starts_with(b"ISA") {
        return Err(invalid(
            "document must begin with a fixed-width ISA segment",
        ));
    }
    let element_separator = bytes[3] as char;
    let component_separator = bytes[104] as char;
    let segment_terminator = bytes[105] as char;
    validate_separator(element_separator, "element")?;
    validate_separator(component_separator, "component")?;
    validate_separator(segment_terminator, "segment")?;
    if element_separator == component_separator
        || element_separator == segment_terminator
        || component_separator == segment_terminator
    {
        return Err(invalid("X12 separators must be distinct"));
    }

    let segments = text
        .split(segment_terminator)
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| parse_segment(segment, element_separator))
        .collect::<Result<Vec<_>, _>>()?;
    if segments.len() > MAX_SEGMENTS {
        return Err(invalid("document contains too many segments"));
    }
    validate_envelope(&segments)?;

    let w05 = exactly_one(&segments, "W05")?;
    if element(w05, 1)? != "N" {
        return Err(invalid(
            "the Wareboxes 940 v1 profile accepts only new shipping orders (W0501=N)",
        ));
    }
    let order_key = element(w05, 2)?.to_owned();
    let destination = destination(&segments)?;
    let lines = lines(&segments)?;
    let rush = segments.iter().any(|segment| {
        segment.tag == "N9" && get(segment, 1) == Some("RU") && get(segment, 2) == Some("Y")
    });
    let ship_by = segments
        .iter()
        .find(|segment| segment.tag == "G62" && get(segment, 1) == Some("10"))
        .map(ship_by)
        .transpose()?;

    Ok(IntegrationOrderEnvelopeRequest {
        order_key,
        rush,
        ship_by,
        destination,
        lines,
    })
}

fn parse_segment(value: &str, separator: char) -> Result<Segment, ParseError> {
    let mut fields = value.split(separator);
    let tag = fields.next().unwrap_or_default().trim();
    if tag.len() < 2 || tag.len() > 3 || !tag.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(invalid("document contains an invalid segment identifier"));
    }
    let elements = fields
        .map(str::trim)
        .map(|field| {
            if field.len() > MAX_ELEMENT_BYTES {
                Err(invalid("document contains an oversized data element"))
            } else {
                Ok(field.to_owned())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if elements.len() > MAX_ELEMENTS_PER_SEGMENT {
        return Err(invalid("document contains too many elements in a segment"));
    }
    Ok(Segment {
        tag: tag.to_owned(),
        elements,
    })
}

fn validate_separator(separator: char, name: &str) -> Result<(), ParseError> {
    if !separator.is_ascii_graphic() || separator.is_ascii_alphanumeric() {
        return Err(invalid(format!("X12 {name} separator is invalid")));
    }
    Ok(())
}

fn validate_envelope(segments: &[Segment]) -> Result<(), ParseError> {
    let tags = segments
        .iter()
        .map(|segment| segment.tag.as_str())
        .collect::<Vec<_>>();
    if tags.first() != Some(&"ISA") || tags.last() != Some(&"IEA") {
        return Err(invalid(
            "document must contain one complete ISA/IEA interchange",
        ));
    }
    for tag in ["ISA", "GS", "ST", "SE", "GE", "IEA"] {
        exactly_one(segments, tag)?;
    }
    let positions = ["ISA", "GS", "ST", "SE", "GE", "IEA"].map(|tag| {
        tags.iter()
            .position(|value| *value == tag)
            .unwrap_or(usize::MAX)
    });
    if !positions.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(invalid("X12 control envelopes are out of order"));
    }
    let isa = exactly_one(segments, "ISA")?;
    let gs = exactly_one(segments, "GS")?;
    let st = exactly_one(segments, "ST")?;
    let se = exactly_one(segments, "SE")?;
    let ge = exactly_one(segments, "GE")?;
    let iea = exactly_one(segments, "IEA")?;
    if element(gs, 1)? != "OW" {
        return Err(invalid(
            "GS01 must identify a warehouse shipping order (OW)",
        ));
    }
    if element(st, 1)? != "940" {
        return Err(invalid("ST01 must be 940"));
    }
    if element(st, 2)? != element(se, 2)? {
        return Err(invalid("ST/SE transaction control numbers do not match"));
    }
    if element(gs, 6)? != element(ge, 2)? {
        return Err(invalid(
            "GS/GE functional group control numbers do not match",
        ));
    }
    if element(isa, 13)? != element(iea, 2)? {
        return Err(invalid("ISA/IEA interchange control numbers do not match"));
    }
    if element(ge, 1)? != "1" || element(iea, 1)? != "1" {
        return Err(invalid(
            "the Wareboxes 940 v1 profile accepts one transaction per interchange",
        ));
    }
    let st_index = tags.iter().position(|tag| *tag == "ST").unwrap_or_default();
    let se_index = tags.iter().position(|tag| *tag == "SE").unwrap_or_default();
    let declared_count = element(se, 1)?
        .parse::<usize>()
        .map_err(|_| invalid("SE01 must be a positive segment count"))?;
    if declared_count == 0 || declared_count != se_index - st_index + 1 {
        return Err(invalid("SE01 does not match the transaction segment count"));
    }
    Ok(())
}

fn destination(segments: &[Segment]) -> Result<FulfillmentOrderDestination, ParseError> {
    let n1_index = segments
        .iter()
        .position(|segment| segment.tag == "N1" && get(segment, 1) == Some("ST"))
        .ok_or_else(|| invalid("ship-to N1 segment is required"))?;
    let n1 = &segments[n1_index];
    let end = segments[n1_index + 1..]
        .iter()
        .position(|segment| matches!(segment.tag.as_str(), "N1" | "LX" | "W01" | "SE"))
        .map_or(segments.len(), |offset| n1_index + 1 + offset);
    let loop_segments = &segments[n1_index + 1..end];
    let n3 = loop_segments
        .iter()
        .find(|segment| segment.tag == "N3")
        .ok_or_else(|| invalid("ship-to N3 address segment is required"))?;
    let n4 = loop_segments
        .iter()
        .find(|segment| segment.tag == "N4")
        .ok_or_else(|| invalid("ship-to N4 geography segment is required"))?;
    let per = loop_segments.iter().find(|segment| segment.tag == "PER");
    Ok(FulfillmentOrderDestination {
        recipient_name: element(n1, 2)?.to_owned(),
        company: None,
        phone: communication(per, "TE"),
        email: communication(per, "EM"),
        line1: element(n3, 1)?.to_owned(),
        line2: nonempty(get(n3, 2)),
        city: element(n4, 1)?.to_owned(),
        region: element(n4, 2)?.to_owned(),
        postal_code: element(n4, 3)?.to_owned(),
        country: element(n4, 4)?.to_owned(),
    })
}

fn communication(segment: Option<&Segment>, qualifier: &str) -> Option<String> {
    let segment = segment?;
    segment
        .elements
        .windows(2)
        .find(|pair| pair[0] == qualifier)
        .and_then(|pair| nonempty(Some(pair[1].as_str())))
}

fn lines(segments: &[Segment]) -> Result<Vec<IntegrationOrderEnvelopeLineRequest>, ParseError> {
    let mut line_key: Option<&str> = None;
    let mut lines = Vec::new();
    for segment in segments {
        match segment.tag.as_str() {
            "LX" => line_key = Some(element(segment, 1)?),
            "W01" => {
                let line_key = line_key
                    .take()
                    .ok_or_else(|| invalid("each W01 line must be preceded by LX"))?;
                let qualifier = element(segment, 4)?;
                if !matches!(qualifier, "SK" | "VP") {
                    return Err(invalid("W0104 must identify an SK or VP item key"));
                }
                lines.push(IntegrationOrderEnvelopeLineRequest {
                    line_key: line_key.to_owned(),
                    external_item_key: element(segment, 5)?.to_owned(),
                    external_uom: element(segment, 2)?.to_owned(),
                    quantity: element(segment, 1)?
                        .parse::<i64>()
                        .map_err(|_| invalid("W0101 must be a positive whole quantity"))?,
                });
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return Err(invalid("at least one LX/W01 demand line is required"));
    }
    Ok(lines)
}

fn ship_by(segment: &Segment) -> Result<String, ParseError> {
    let date = NaiveDate::parse_from_str(element(segment, 2)?, "%Y%m%d")
        .map_err(|_| invalid("G6202 must use CCYYMMDD"))?;
    let time = match get(segment, 3).filter(|value| !value.is_empty()) {
        Some(value) if value.len() == 4 => NaiveTime::parse_from_str(value, "%H%M"),
        Some(value) if value.len() == 6 => NaiveTime::parse_from_str(value, "%H%M%S"),
        Some(_) => return Err(invalid("G6203 must use HHMM or HHMMSS")),
        None => Ok(NaiveTime::MIN),
    }
    .map_err(|_| invalid("G6203 contains an invalid time"))?;
    Ok(Utc.from_utc_datetime(&date.and_time(time)).to_rfc3339())
}

fn exactly_one<'a>(segments: &'a [Segment], tag: &str) -> Result<&'a Segment, ParseError> {
    let mut matches = segments.iter().filter(|segment| segment.tag == tag);
    let first = matches
        .next()
        .ok_or_else(|| invalid(format!("{tag} segment is required")))?;
    if matches.next().is_some() {
        return Err(invalid(format!("only one {tag} segment is supported")));
    }
    Ok(first)
}

fn element(segment: &Segment, position: usize) -> Result<&str, ParseError> {
    get(segment, position)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("{}{:02} is required", segment.tag, position)))
}

fn get(segment: &Segment, position: usize) -> Option<&str> {
    position
        .checked_sub(1)
        .and_then(|index| segment.elements.get(index))
        .map(String::as_str)
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_owned)
}

fn invalid(message: impl Into<String>) -> ParseError {
    ParseError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = concat!(
        "ISA*00*          *00*          *ZZ*SENDER         *ZZ*WAREBOXES      *260812*1200*U*00401*000000001*0*P*>~",
        "GS*OW*SENDER*WAREBOXES*20260812*1200*1*X*004010~",
        "ST*940*0001~",
        "W05*N*SO-1001~",
        "N9*RU*Y~",
        "G62*10*20260813*173000~",
        "N1*ST*Receiving Team~",
        "N3*125 Shipping Lane*Dock 4~",
        "N4*Reno*NV*89502*US~",
        "PER*CN*Receiving Team*TE*7755550100*EM*receiving@example.test~",
        "LX*1~",
        "W01*4*CS**SK*CLIENT-CASE~",
        "SE*11*0001~",
        "GE*1*1~",
        "IEA*1*000000001~",
    );

    #[test]
    fn maps_the_supported_940_profile_to_canonical_order_demand() {
        assert_eq!(DOCUMENT.as_bytes()[105], b'~');
        let order = parse(DOCUMENT.as_bytes()).unwrap();
        assert_eq!(order.order_key, "SO-1001");
        assert!(order.rush);
        assert_eq!(order.ship_by.as_deref(), Some("2026-08-13T17:30:00+00:00"));
        assert_eq!(order.destination.city, "Reno");
        assert_eq!(order.destination.phone.as_deref(), Some("7755550100"));
        assert_eq!(order.lines.len(), 1);
        assert_eq!(order.lines[0].external_item_key, "CLIENT-CASE");
        assert_eq!(order.lines[0].quantity, 4);
    }

    #[test]
    fn rejects_wrong_control_counts_and_non_new_actions() {
        assert!(parse(DOCUMENT.replace("SE*11", "SE*10").as_bytes()).is_err());
        assert!(parse(DOCUMENT.replace("W05*N", "W05*R").as_bytes()).is_err());
    }
}
