use std::collections::HashSet;

use crate::{FulfillmentOrderDemandLine, OrderRevision, OrderStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderLineAmendmentTransition {
    pub revision: OrderRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrderLineAmendmentError {
    #[error("only open or held orders can replace demand lines before physical execution")]
    InvalidOrderStatus,
    #[error("a fulfillment order requires at least one demand line")]
    MissingDemandLines,
    #[error("order line key must be unique within the order: {line_key}")]
    DuplicateLineKey { line_key: String },
    #[error("order line replacement must change demand or line sequence")]
    NoChanges,
    #[error("order revision overflow")]
    RevisionOverflow,
}

pub fn replace_fulfillment_order_lines(
    status: OrderStatus,
    revision: OrderRevision,
    current: &[FulfillmentOrderDemandLine],
    requested: &[FulfillmentOrderDemandLine],
) -> Result<OrderLineAmendmentTransition, OrderLineAmendmentError> {
    if !matches!(status, OrderStatus::Open | OrderStatus::Held) {
        return Err(OrderLineAmendmentError::InvalidOrderStatus);
    }
    if requested.is_empty() {
        return Err(OrderLineAmendmentError::MissingDemandLines);
    }
    let mut line_keys = HashSet::with_capacity(requested.len());
    for line in requested {
        if !line_keys.insert(line.line_key().as_str()) {
            return Err(OrderLineAmendmentError::DuplicateLineKey {
                line_key: line.line_key().as_str().to_owned(),
            });
        }
    }
    if current == requested {
        return Err(OrderLineAmendmentError::NoChanges);
    }
    let revision = revision
        .checked_next()
        .ok_or(OrderLineAmendmentError::RevisionOverflow)?;
    Ok(OrderLineAmendmentTransition { revision })
}

#[cfg(test)]
mod tests {
    use crate::{CatalogItemId, OrderLineKey, OrderQuantity, RequestedUom};

    use super::*;

    fn line(key: &str, item: i64, quantity: i64) -> FulfillmentOrderDemandLine {
        FulfillmentOrderDemandLine::new(
            OrderLineKey::new(key).unwrap(),
            CatalogItemId::new(item).unwrap(),
            OrderQuantity::new(quantity).unwrap(),
            RequestedUom::new("case").unwrap(),
        )
    }

    #[test]
    fn open_and_held_orders_accept_an_exact_changed_line_set() {
        for status in [OrderStatus::Open, OrderStatus::Held] {
            let transition = replace_fulfillment_order_lines(
                status,
                OrderRevision::new(7).unwrap(),
                &[line("1", 11, 2)],
                &[line("1", 11, 3), line("2", 12, 1)],
            )
            .unwrap();
            assert_eq!(transition.revision.get(), 8);
        }
    }

    #[test]
    fn empty_duplicate_noop_and_execution_states_are_rejected() {
        let current = vec![line("1", 11, 2)];
        assert_eq!(
            replace_fulfillment_order_lines(
                OrderStatus::Open,
                OrderRevision::new(1).unwrap(),
                &current,
                &[],
            ),
            Err(OrderLineAmendmentError::MissingDemandLines)
        );
        assert_eq!(
            replace_fulfillment_order_lines(
                OrderStatus::Open,
                OrderRevision::new(1).unwrap(),
                &current,
                &[line("1", 11, 2), line("1", 12, 1)],
            ),
            Err(OrderLineAmendmentError::DuplicateLineKey {
                line_key: "1".to_owned()
            })
        );
        assert_eq!(
            replace_fulfillment_order_lines(
                OrderStatus::Open,
                OrderRevision::new(1).unwrap(),
                &current,
                &current,
            ),
            Err(OrderLineAmendmentError::NoChanges)
        );
        assert_eq!(
            replace_fulfillment_order_lines(
                OrderStatus::Processing,
                OrderRevision::new(1).unwrap(),
                &current,
                &[line("1", 11, 3)],
            ),
            Err(OrderLineAmendmentError::InvalidOrderStatus)
        );
    }
}
