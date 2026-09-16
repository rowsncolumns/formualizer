//! Financial functions
//! Functions implemented: PMT, PV, FV, NPV, NPER, RATE, IPMT, PPMT, CUMIPMT, CUMPRINC, SLN, SYD,
//! DB, DDB, VDB, XNPV, XIRR, FVSCHEDULE, DOLLARDE, DOLLARFR, ACCRINT, ACCRINTM, PRICE, YIELD,
//! COUPDAYBS, COUPDAYS, COUPDAYSNC, COUPNCD, COUPPCD, COUPNUM, DURATION, MDURATION, DISC, PRICEDISC,
//! PRICEMAT, YIELDDISC, YIELDMAT, INTRATE, RECEIVED, ODDFPRICE, ODDFYIELD, ODDLPRICE, ODDLYIELD,
//! AMORLINC, AMORDEGRC, TBILLEQ, TBILLPRICE, TBILLYIELD, ISPMT, PDURATION

mod bonds;
mod depreciation;
mod tvm;

pub use bonds::*;
pub use depreciation::*;
pub use tvm::*;

pub fn register_builtins() {
    bonds::register_builtins();
    tvm::register_builtins();
    depreciation::register_builtins();
}
