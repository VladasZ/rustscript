//! The scalar receivers, `bool`, `char`, `json` and the named bridge types.

use std::sync::Arc;

use syn::Expr;

use super::{Infer, Ty};
use crate::interpreter::numeric::IntWidth;

impl Infer<'_, '_> {
    pub(super) fn int_method(&mut self, recv: &Ty, name: &str, args: &[&Expr]) -> Ty {
        match name {
            "abs" | "pow" | "signum" | "isqrt" | "saturating_add" | "saturating_sub"
            | "saturating_mul" | "saturating_pow" | "wrapping_add" | "wrapping_sub"
            | "wrapping_mul" | "wrapping_neg" | "wrapping_pow" | "wrapping_shl"
            | "wrapping_shr" | "rotate_left" | "rotate_right" | "rem_euclid" | "div_euclid"
            | "midpoint" | "min" | "max" | "clamp" | "swap_bytes" | "reverse_bits" | "to_be"
            | "to_le" | "abs_diff" | "next_power_of_two" | "ilog2" | "ilog10" | "div_ceil"
            | "next_multiple_of" | "unsigned_abs" => {
                let want = if matches!(
                    name,
                    "pow"
                        | "saturating_pow"
                        | "wrapping_pow"
                        | "rotate_left"
                        | "rotate_right"
                        | "wrapping_shl"
                        | "wrapping_shr"
                ) {
                    Ty::Int(IntWidth::U32)
                } else {
                    recv.clone()
                };
                for arg in args {
                    self.expr(arg, &want);
                }
                match name {
                    "ilog2" | "ilog10" => Ty::Int(IntWidth::U32),
                    _ => recv.clone(),
                }
            }
            "checked_add"
            | "checked_sub"
            | "checked_mul"
            | "checked_div"
            | "checked_rem"
            | "checked_neg"
            | "checked_abs"
            | "checked_pow"
            | "checked_shl"
            | "checked_shr"
            | "checked_div_euclid"
            | "checked_rem_euclid"
            | "checked_next_power_of_two"
            | "checked_ilog2"
            | "checked_ilog10" => {
                let want = if matches!(name, "checked_pow" | "checked_shl" | "checked_shr") {
                    Ty::Int(IntWidth::U32)
                } else {
                    recv.clone()
                };
                for arg in args {
                    self.expr(arg, &want);
                }
                Ty::option(recv.clone())
            }
            "overflowing_add" | "overflowing_sub" | "overflowing_mul" => {
                for arg in args {
                    self.expr(arg, recv);
                }
                Ty::Tuple(vec![recv.clone(), Ty::Bool])
            }
            "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "leading_ones"
            | "trailing_ones" => Ty::Int(IntWidth::U32),
            "is_positive" | "is_negative" | "is_power_of_two" => Ty::Bool,
            "to_be_bytes" | "to_le_bytes" | "to_ne_bytes" => Ty::vec(Ty::Int(IntWidth::U8)),
            "to_string" | "as_str" => Ty::Str,
            "sqrt" | "powf" | "powi" => Ty::F64,
            _ => self.walk_all(args),
        }
    }

    pub(super) fn float_method(&mut self, recv: &Ty, name: &str, args: &[&Expr]) -> Ty {
        match name {
            "abs" | "sqrt" | "cbrt" | "floor" | "ceil" | "round" | "trunc" | "fract" | "signum"
            | "powi" | "powf" | "exp" | "exp2" | "ln" | "log" | "log2" | "log10" | "sin"
            | "cos" | "tan" | "asin" | "acos" | "atan" | "atan2" | "sinh" | "cosh" | "tanh"
            | "hypot" | "min" | "max" | "clamp" | "mul_add" | "recip" | "to_degrees"
            | "to_radians" | "copysign" | "rem_euclid" | "div_euclid" | "midpoint"
            | "round_ties_even" => {
                let want = if name == "powi" {
                    Ty::Int(IntWidth::I32)
                } else {
                    recv.clone()
                };
                for arg in args {
                    self.expr(arg, &want);
                }
                recv.clone()
            }
            "is_nan" | "is_finite" | "is_infinite" | "is_sign_negative" | "is_sign_positive"
            | "is_normal" => Ty::Bool,
            "to_bits" => Ty::Int(if matches!(recv, Ty::F32) {
                IntWidth::U32
            } else {
                IntWidth::U64
            }),
            "total_cmp" => {
                self.walk_all(args);
                Ty::named("Ordering")
            }
            _ => self.walk_all(args),
        }
    }

    pub(super) fn bool_method(&mut self, name: &str, args: &[&Expr], expected: &Ty) -> Ty {
        match name {
            "then_some" => {
                let payload = self.arg_ty(args, 0, &expected.payload());
                Ty::option(payload)
            }
            "then" => {
                let payload = self.closure_ret_expecting(args, 0, Vec::new(), &expected.payload());
                Ty::option(payload)
            }
            "not" => Ty::Bool,
            _ => self.walk_all(args),
        }
    }

    pub(super) fn char_method(&mut self, name: &str, args: &[&Expr]) -> Ty {
        match name {
            "is_alphabetic"
            | "is_numeric"
            | "is_alphanumeric"
            | "is_whitespace"
            | "is_uppercase"
            | "is_lowercase"
            | "is_ascii"
            | "is_ascii_digit"
            | "is_ascii_alphabetic"
            | "is_ascii_alphanumeric"
            | "is_ascii_uppercase"
            | "is_ascii_lowercase"
            | "is_ascii_punctuation"
            | "is_ascii_whitespace"
            | "is_ascii_hexdigit"
            | "is_digit"
            | "is_control"
            | "is_ascii_graphic"
            | "eq_ignore_ascii_case" => {
                self.walk_all(args);
                Ty::Bool
            }
            "to_digit" => {
                self.arg_ty(args, 0, &Ty::Int(IntWidth::U32));
                Ty::option(Ty::Int(IntWidth::U32))
            }
            "to_ascii_uppercase" | "to_ascii_lowercase" => Ty::Char,
            "to_uppercase" | "to_lowercase" => Ty::iter(Ty::Char),
            "to_string" => Ty::Str,
            "len_utf8" => Ty::usize(),
            _ => self.walk_all(args),
        }
    }

    pub(super) fn json_method(&mut self, name: &str, args: &[&Expr]) -> Ty {
        self.walk_all(args);
        match name {
            "get" | "get_mut" | "pointer" | "pointer_mut" => Ty::option(Ty::Json),
            "as_str" => Ty::option(Ty::Str),
            "as_i64" => Ty::option(Ty::Int(IntWidth::I64)),
            "as_u64" => Ty::option(Ty::Int(IntWidth::U64)),
            "as_f64" => Ty::option(Ty::F64),
            "as_bool" => Ty::option(Ty::Bool),
            "as_array" | "as_array_mut" => Ty::option(Ty::vec(Ty::Json)),
            "as_object" | "as_object_mut" => {
                Ty::option(Ty::Map(Box::new(Ty::Str), Box::new(Ty::Json)))
            }
            "is_null" | "is_string" | "is_number" | "is_boolean" | "is_array" | "is_object"
            | "is_i64" | "is_u64" | "is_f64" => Ty::Bool,
            "to_string" => Ty::Str,
            "take" | "clone" => Ty::Json,
            _ => Ty::Unknown,
        }
    }

    /// The regex, path and file types, grouped by what a method gives back.
    fn file_named_method(kind: &str, name: &str) -> Option<Ty> {
        let io = |ok: Ty| Ty::result(ok, Ty::named("io::Error"));
        Some(match (kind, name) {
            ("Regex", "is_match")
            | (
                "Path" | "PathBuf",
                "exists" | "is_dir" | "is_file" | "is_absolute" | "is_relative" | "starts_with"
                | "ends_with" | "is_symlink",
            )
            | ("Metadata", "is_dir" | "is_file" | "is_symlink")
            | ("FileType", _)
            | ("ExitStatus", "success")
            | ("StatusCode", "is_success" | "is_client_error" | "is_server_error")
            | ("Ordering", "is_lt" | "is_le" | "is_gt" | "is_ge" | "is_eq" | "is_ne")
            | ("JoinHandle", "is_finished") => Ty::Bool,
            ("Regex", "replace" | "replace_all" | "replacen" | "as_str")
            | ("Match", "as_str")
            | ("Path" | "PathBuf", "to_string_lossy" | "display")
            | ("DirEntry", "file_name")
            | ("DateTime", "to_rfc3339")
            | ("DelayedFormat", "to_string")
            | ("Uuid", "to_string" | "simple" | "hyphenated") => Ty::Str,
            ("Captures", "len") | ("Match", "start" | "end" | "len") => Ty::usize(),
            ("Regex", "split") | ("Path" | "PathBuf", "components" | "ancestors" | "iter") => {
                Ty::iter(Ty::Str)
            }
            ("Regex", "captures") => Ty::option(Ty::named("Captures")),
            ("Regex", "find") | ("Captures", "get" | "name") => Ty::option(Ty::named("Match")),
            ("Regex", "find_iter") => Ty::iter(Ty::named("Match")),
            ("Regex", "captures_iter") => Ty::iter(Ty::named("Captures")),
            ("Match", "range") => Ty::Range(Box::new(Ty::usize())),
            ("Path" | "PathBuf", "to_str" | "file_name" | "extension" | "file_stem") => {
                Ty::option(Ty::Str)
            }
            ("Path" | "PathBuf", "parent") => Ty::option(Ty::named("Path")),
            (
                "Path" | "PathBuf",
                "join" | "with_extension" | "with_file_name" | "to_path_buf" | "as_path" | "clone",
            )
            | ("DirEntry", "path") => Ty::named("PathBuf"),
            ("Path" | "PathBuf", "canonicalize") => io(Ty::named("PathBuf")),
            ("Path" | "PathBuf", "push" | "set_extension" | "pop") | ("JoinHandle", "abort") => {
                Ty::Unit
            }
            ("Path" | "PathBuf", "read_dir") => io(Ty::iter(io(Ty::named("DirEntry")))),
            ("Path" | "PathBuf", "metadata") => io(Ty::named("Metadata")),
            ("DirEntry", "file_type" | "metadata") => io(Ty::named(name)),
            ("Metadata", "len") | ("Duration", "as_secs") => Ty::Int(IntWidth::U64),
            ("Metadata", "modified" | "created" | "accessed") => io(Ty::named("SystemTime")),
            _ => return None,
        })
    }

    /// The bridge types, grouped by what a method gives back.
    pub(super) fn named_method(
        &mut self,
        kind: &Arc<str>,
        name: &str,
        args: &[&Expr],
        expected: &Ty,
    ) -> Ty {
        let io = |ok: Ty| Ty::result(ok, Ty::named("io::Error"));
        if let Some(ty) = Self::file_named_method(kind, name) {
            self.walk_all(args);
            return ty;
        }
        let ty = match (&**kind, name) {
            ("Duration", "as_millis" | "as_micros" | "as_nanos") => Ty::Int(IntWidth::U128),
            ("Duration", "as_secs_f64") => Ty::F64,
            ("Duration", "as_secs_f32") => Ty::F32,
            ("Duration", "subsec_millis" | "subsec_micros" | "subsec_nanos")
            | ("Child", "id")
            | ("DateTime", "month" | "day" | "hour" | "minute" | "second" | "ordinal") => {
                Ty::Int(IntWidth::U32)
            }
            ("Instant", "elapsed" | "duration_since") => Ty::named("Duration"),
            ("SystemTime", "duration_since" | "elapsed") => {
                Ty::result(Ty::named("Duration"), Ty::named("SystemTimeError"))
            }
            (
                "Command",
                "arg" | "args" | "current_dir" | "env" | "envs" | "stdin" | "stdout" | "stderr"
                | "env_remove" | "env_clear",
            ) => Ty::named("Command"),
            ("Command", "output") | ("Child", "wait_with_output") => io(Ty::named("Output")),
            ("Command", "status") | ("Child", "wait") => io(Ty::named("ExitStatus")),
            ("Command", "spawn") => io(Ty::named("Child")),
            ("Output", "status") => Ty::named("ExitStatus"),
            ("Output", "stdout" | "stderr") => Ty::vec(Ty::Int(IntWidth::U8)),
            ("ExitStatus", "code") => Ty::option(Ty::Int(IntWidth::I32)),
            ("Child", "kill")
            | ("File" | "Stdout" | "Stderr" | "BufWriter", "write_all" | "flush" | "sync_all") => {
                io(Ty::Unit)
            }
            ("Stdin", "lock") => Ty::named("Stdin"),
            ("Stdin" | "File" | "BufReader", "lines") => Ty::iter(io(Ty::Str)),
            ("Stdin" | "File" | "BufReader", "read_line" | "read_to_string") => io(Ty::usize()),
            ("Response", "status") => Ty::named("StatusCode"),
            ("Response", "text") => Ty::result(Ty::Str, Ty::named("reqwest::Error")),
            ("Response", "bytes") => {
                Ty::result(Ty::vec(Ty::Int(IntWidth::U8)), Ty::named("reqwest::Error"))
            }
            ("Response", "json") => Ty::result(expected.payload(), Ty::named("reqwest::Error")),
            ("Response", "headers") => Ty::named("HeaderMap"),
            ("StatusCode", "as_u16") => Ty::Int(IntWidth::U16),
            (
                "Client" | "ClientBuilder",
                "get" | "post" | "put" | "delete" | "patch" | "head" | "request",
            )
            | ("RequestBuilder", _)
                if name != "send" =>
            {
                Ty::named("RequestBuilder")
            }
            ("RequestBuilder", "send") => {
                Ty::result(Ty::named("Response"), Ty::named("reqwest::Error"))
            }
            ("ClientBuilder", "build") => {
                Ty::result(Ty::named("Client"), Ty::named("reqwest::Error"))
            }
            ("ClientBuilder", _) => Ty::named("ClientBuilder"),
            ("DateTime", "format") => Ty::named("DelayedFormat"),
            ("DateTime", "timestamp" | "timestamp_millis") => Ty::Int(IntWidth::I64),
            ("DateTime", "year") => Ty::Int(IntWidth::I32),
            ("Ordering", "then" | "then_with" | "reverse") => Ty::named("Ordering"),
            _ => Ty::Unknown,
        };
        self.walk_all(args);
        ty
    }
}
