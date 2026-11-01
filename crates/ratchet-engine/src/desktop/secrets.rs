//! Where secret values live. The stores moved to `ratchet-connectors` (connectors keep their tokens and keys there too);
//! this module keeps the old path working.

pub use ratchet_connectors::secrets::*;
