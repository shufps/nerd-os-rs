//! Chip drivers (regulators, fan controllers, sensors, the DAC, the IO expander, the W5500) and the I2C and UART buses they use.

pub mod ds4432u;
pub mod emc2101;
pub mod emc2302;
pub mod fxl6408;
pub mod i2c;
pub mod ina260;
pub mod serial;
pub mod tmp1075;
pub mod tmp451;
pub mod tps536x7;
pub mod tps546;
pub mod w5500;
