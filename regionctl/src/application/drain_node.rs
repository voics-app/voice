//! Use cases: drain and undrain a live voice node through the gateway. The gateway stops
//! allocating the node and marks it for the node itself (`/discordy/voice/drain/<node_id>`), so
//! existing calls continue and new ones go elsewhere.

use super::error::AppError;
use super::ports::NodeDrainGateway;
use crate::domain::region::NodeId;

/// Drains (`true`) or undrains (`false`) `node_id`.
pub fn set_node_drain(
    gateway: &dyn NodeDrainGateway,
    node_id: &str,
    drained: bool,
) -> Result<(), AppError> {
    let node = NodeId::parse(node_id)?;
    gateway.set_drained(&node, drained)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<(String, bool)>>);

    impl NodeDrainGateway for Recorder {
        fn set_drained(&self, node: &NodeId, drained: bool) -> Result<(), AppError> {
            self.0.borrow_mut().push((node.to_string(), drained));
            Ok(())
        }
    }

    #[test]
    fn forwards_valid_node_ids() {
        let gateway = Recorder::default();
        set_node_drain(&gateway, "eu-west-1", true).unwrap();
        set_node_drain(&gateway, "eu-west-1", false).unwrap();
        assert_eq!(
            *gateway.0.borrow(),
            vec![
                ("eu-west-1".to_owned(), true),
                ("eu-west-1".to_owned(), false)
            ]
        );
    }

    #[test]
    fn rejects_ids_that_could_alter_the_request_path() {
        let gateway = Recorder::default();
        for bad in ["", "a/b", "../x", "A", "a b", "x?y", &"n".repeat(64)] {
            assert!(set_node_drain(&gateway, bad, true).is_err(), "{bad}");
        }
        assert!(gateway.0.borrow().is_empty());
    }
}
