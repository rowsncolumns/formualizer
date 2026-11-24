pub mod database; // Phase 4 database functions (DSUM, DAVERAGE, etc.)
pub mod datetime; // Phase 3 date and time functions
pub mod engineering; // Phase 2 engineering functions (bitwise, base conversion, complex numbers, etc.)
pub mod external; // External-service functions (CUBE*, RTD, STOCKHISTORY, WEBSERVICE, …) + FILTERXML
pub mod financial; // Phase 5 financial functions
pub mod info; // Sprint 9 info / error introspection
pub mod lambda;
pub mod logical;
pub mod logical_ext;
pub mod lookup; // Sprint 4 classic lookup (partial)
pub mod math;
pub mod random;
pub mod reference_fns;
pub mod sheets_extras; // Google Sheets extras the legacy JS engine carries (SPLIT, JOIN, REGEXMATCH, SORTN, ARRAYFORMULA, QUERY stub)
pub mod stats; // Phase 6 statistical basics + extended stats
pub mod text; // Phase 2 core text functions
pub(crate) mod utils;

#[cfg(test)]
mod tests;

/// How many times the whole builtin library has been registered in this process.
#[cfg(test)]
pub(crate) static BUILTIN_LOAD_PASSES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

static BUILTINS_LOADED: once_cell::sync::OnceCell<()> = once_cell::sync::OnceCell::new();

/// Register the builtin library. Idempotent and thread-safe: the registration pass runs once
/// per process, so callers on hot paths (one formula's planning, template canonicalization)
/// pay one atomic load. Nothing is ever removed from the registry, and a trusted builtin only
/// fills vacant slots, so a second pass could not change it.
pub fn load_builtins() {
    BUILTINS_LOADED.get_or_init(|| {
        #[cfg(test)]
        BUILTIN_LOAD_PASSES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        database::register_builtins();
        datetime::register_builtins();
        engineering::register_builtins();
        external::register_builtins();
        financial::register_builtins();
        lambda::register_builtins();
        logical::register_builtins();
        logical_ext::register_builtins();
        info::register_builtins();
        math::register_builtins();
        random::register_builtins();
        reference_fns::register_builtins();
        sheets_extras::register_builtins();
        lookup::register_builtins();
        text::register_builtins();
        stats::register_builtins();
    });
}
