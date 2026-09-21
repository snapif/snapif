use std::sync::Arc;

use crate::wire::Usage;

pub type UsageFn = Arc<dyn Fn(Usage) + Send + Sync>;
