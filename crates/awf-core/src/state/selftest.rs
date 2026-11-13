//! `awf state --selftest`（移植中）。

use super::Env;

pub fn selftest(env: &Env, out: &mut String) -> i32 {
    let _ = env;
    out.push_str("awf selftest：還沒移植\n");
    1
}
