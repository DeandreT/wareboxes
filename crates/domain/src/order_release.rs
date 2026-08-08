use crate::OrderStatus;

/// Applies the only order-state transition accepted by waveless release.
pub const fn release_order(status: OrderStatus) -> Result<OrderStatus, OrderReleaseError> {
    match status {
        OrderStatus::Open => Ok(OrderStatus::Processing),
        _ => Err(OrderReleaseError::OrderNotOpen { status }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OrderReleaseError {
    #[error("only an open order can be released, got {status}")]
    OrderNotOpen { status: OrderStatus },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_open_orders_transition_to_processing() {
        assert_eq!(
            release_order(OrderStatus::Open),
            Ok(OrderStatus::Processing)
        );

        for status in OrderStatus::ALL {
            if status == OrderStatus::Open {
                continue;
            }
            assert_eq!(
                release_order(status),
                Err(OrderReleaseError::OrderNotOpen { status })
            );
        }
    }
}
