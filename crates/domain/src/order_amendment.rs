use crate::{OrderRevision, OrderStatus, ShippingDestination, Timestamp};

/// Complete mutable fulfillment header used for optimistic replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FulfillmentOrderHeader {
    rush: bool,
    ship_by: Option<Timestamp>,
    destination: ShippingDestination,
}

impl FulfillmentOrderHeader {
    pub const fn new(
        rush: bool,
        ship_by: Option<Timestamp>,
        destination: ShippingDestination,
    ) -> Self {
        Self {
            rush,
            ship_by,
            destination,
        }
    }

    pub const fn rush(&self) -> bool {
        self.rush
    }

    pub const fn ship_by(&self) -> Option<&Timestamp> {
        self.ship_by.as_ref()
    }

    pub const fn destination(&self) -> &ShippingDestination {
        &self.destination
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderAmendmentTransition {
    pub revision: OrderRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OrderAmendmentError {
    #[error("only open or held orders can be amended before physical execution")]
    InvalidOrderStatus,
    #[error("order amendment must change rush, ship-by, or destination")]
    NoChanges,
    #[error("order revision overflow")]
    RevisionOverflow,
}

pub fn amend_fulfillment_order(
    status: OrderStatus,
    revision: OrderRevision,
    current: &FulfillmentOrderHeader,
    requested: &FulfillmentOrderHeader,
) -> Result<OrderAmendmentTransition, OrderAmendmentError> {
    if !matches!(status, OrderStatus::Open | OrderStatus::Held) {
        return Err(OrderAmendmentError::InvalidOrderStatus);
    }
    if current == requested {
        return Err(OrderAmendmentError::NoChanges);
    }
    let revision = revision
        .checked_next()
        .ok_or(OrderAmendmentError::RevisionOverflow)?;
    Ok(OrderAmendmentTransition { revision })
}

#[cfg(test)]
mod tests {
    use crate::{ShippingRecipient, Timestamp};

    use super::*;

    fn destination(line1: &str) -> ShippingDestination {
        ShippingDestination::new(
            ShippingRecipient::new("Receiving", None, None, None).unwrap(),
            line1,
            None,
            "Reno",
            "NV",
            "89502",
            "US",
        )
        .unwrap()
    }

    #[test]
    fn open_and_held_headers_advance_exactly_once() {
        let current = FulfillmentOrderHeader::new(false, None, destination("100 Old Way"));
        let requested = FulfillmentOrderHeader::new(
            true,
            Some("2027-08-12T17:00:00Z".parse::<Timestamp>().unwrap()),
            destination("200 New Way"),
        );

        for status in [OrderStatus::Open, OrderStatus::Held] {
            let transition = amend_fulfillment_order(
                status,
                OrderRevision::new(4).unwrap(),
                &current,
                &requested,
            )
            .unwrap();
            assert_eq!(transition.revision.get(), 5);
        }
    }

    #[test]
    fn physical_execution_and_noop_changes_are_rejected() {
        let current = FulfillmentOrderHeader::new(false, None, destination("100 Old Way"));
        assert_eq!(
            amend_fulfillment_order(
                OrderStatus::Processing,
                OrderRevision::new(4).unwrap(),
                &current,
                &FulfillmentOrderHeader::new(true, None, destination("100 Old Way")),
            ),
            Err(OrderAmendmentError::InvalidOrderStatus)
        );
        assert_eq!(
            amend_fulfillment_order(
                OrderStatus::Open,
                OrderRevision::new(4).unwrap(),
                &current,
                &current,
            ),
            Err(OrderAmendmentError::NoChanges)
        );
    }
}
