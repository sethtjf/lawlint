//! Declarative rule engines. Each engine is a struct implementing `Rule`,
//! constructed from a validated `RuleDef` (+ its `RuleMeta`).
//!
//! Derived tier: phrase/leading → static; density/statistical → statistical;
//! inferential → inferential (no runtime engine; carries a RubricFragment).

pub mod density;
pub mod leading;
pub mod phrase;
pub mod statistical;

pub use density::DensityEngine;
pub use leading::LeadingEngine;
pub use phrase::PhraseEngine;
pub use statistical::StatisticalEngine;
