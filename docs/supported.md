# Supported interpreter surface

Generated from the bridge tables, do not edit by hand. Run
`rust supported md > docs/supported.md` after changing a bridge.
The `supported_page_is_current` test enforces it.

## any value

`abs`, `abs_diff`, `acos`, `as_array`, `as_array_mut`, `as_bool`, `as_f64`, `as_i128`, `as_i64`, `as_object`, `as_object_mut`, `as_str`, `as_u64`, `as_usize`, `asin`, `atan`, `atan2`, `cbrt`, `ceil`, `checked_abs`, `checked_add`, `checked_div`, `checked_ilog2`, `checked_mul`, `checked_neg`, `checked_pow`, `checked_rem`, `checked_rem_euclid`, `checked_shl`, `checked_shr`, `checked_sub`, `clamp`, `clear`, `clone`, `cmp`, `contains`, `copysign`, `cos`, `cosh`, `count`, `count_ones`, `count_zeros`, `div_ceil`, `div_euclid`, `exp`, `exp2`, `extend`, `extend_from_slice`, `floor`, `fold`, `fract`, `get`, `hypot`, `ilog10`, `ilog2`, `into`, `is_array`, `is_boolean`, `is_empty`, `is_f64`, `is_finite`, `is_i64`, `is_infinite`, `is_multiple_of`, `is_nan`, `is_negative`, `is_normal`, `is_null`, `is_number`, `is_object`, `is_positive`, `is_power_of_two`, `is_sign_negative`, `is_sign_positive`, `is_string`, `is_subnormal`, `is_u64`, `isqrt`, `leading_ones`, `leading_zeros`, `len`, `ln`, `log10`, `log2`, `make_ascii_lowercase`, `make_ascii_uppercase`, `max`, `midpoint`, `min`, `mode`, `mul_add`, `next_multiple_of`, `next_power_of_two`, `overflowing_add`, `overflowing_mul`, `overflowing_sub`, `partial_cmp`, `pointer`, `pointer_mut`, `pow`, `powf`, `powi`, `push`, `push_str`, `readonly`, `recip`, `rem_euclid`, `reverse_bits`, `rotate_left`, `rotate_right`, `round`, `round_ties_even`, `saturating_add`, `saturating_mul`, `saturating_pow`, `saturating_sub`, `set_readonly`, `signum`, `sin`, `sinh`, `sqrt`, `swap_bytes`, `tan`, `tanh`, `then`, `then_some`, `then_with`, `to_be_bytes`, `to_degrees`, `to_le_bytes`, `to_ne_bytes`, `to_radians`, `to_string`, `total_cmp`, `trailing_ones`, `trailing_zeros`, `trunc`, `wrapping_abs`, `wrapping_add`, `wrapping_mul`, `wrapping_neg`, `wrapping_pow`, `wrapping_shl`, `wrapping_shr`, `wrapping_sub`

## Base64

`decode`, `encode`, `kind`, `standard_no_pad`, `url_safe`, `url_safe_no_pad`

## Block

`border_style`, `border_type`, `bordered`, `borders`, `padding`, `style`, `title`

## Buffer

`area`, `cell`, `content`

## BufferCell

`symbol`

## Builder

`blocking`, `build`, `cookie_store`, `redirect`, `timeout`, `user_agent`

## Captures

`get`, `len`, `name`

## Cell

`blocking_lock`, `borrow`, `borrow_mut`, `clone`, `get`, `get_mut`, `into_inner`, `lock`, `replace`, `set`, `style`, `take`, `try_borrow`, `try_borrow_mut`, `try_lock`

## Char

`eq_ignore_ascii_case`, `is_alphabetic`, `is_alphanumeric`, `is_ascii`, `is_ascii_alphabetic`, `is_ascii_alphanumeric`, `is_ascii_digit`, `is_ascii_hexdigit`, `is_ascii_lowercase`, `is_ascii_punctuation`, `is_ascii_uppercase`, `is_ascii_whitespace`, `is_control`, `is_lowercase`, `is_numeric`, `is_uppercase`, `is_whitespace`, `len_utf8`, `to_ascii_lowercase`, `to_ascii_uppercase`, `to_digit`, `to_lowercase`, `to_uppercase`

## Child

`status`, `stderr`, `stdin`, `stdout`, `wait`, `wait_with_output`

## Client

`clone`, `delete`, `get`, `head`, `patch`, `post`, `put`

## Command

`arg`, `args`, `current_dir`, `cwd`, `env`, `env_remove`, `output`, `spawn`, `status`, `stderr`, `stdin`, `stdout`

## DateTime

`day`, `format`, `hour`, `local`, `minute`, `month`, `nanos`, `offset`, `second`, `secs`, `timestamp`, `timestamp_millis`, `to_rfc3339`, `year`

## DirEntry

`file_name`, `file_type`, `name`, `path`

## Document

`change_page_content`, `get_page_content`, `get_pages`, `save`

## Duration

`as_micros`, `as_millis`, `as_nanos`, `as_secs`, `as_secs_f64`, `checked_add`, `checked_sub`, `is_zero`, `nanos`, `secs`, `subsec_micros`, `subsec_millis`, `subsec_nanos`

## Element

`get_text`, `write`

## Entry

`and_modify`, `key`, `or_default`, `or_insert`, `or_insert_with`, `or_insert_with_key`

## ExitStatus

`code`, `success`

## FileType

`is_dir`, `is_file`, `is_symlink`

## HeaderMap

`get`, `map`, `text`

## HeaderValue

`as_str`, `as_string`, `to_str`, `to_string`

## Iterator

`all`, `any`, `as_str`, `by_ref`, `chain`, `cloned`, `collect`, `collect_map`, `collect_set`, `collect_string`, `copied`, `count`, `enumerate`, `filter`, `filter_map`, `find`, `find_map`, `for_each`, `last`, `map`, `max`, `min`, `next`, `next_back`, `nth`, `peek`, `peekable`, `position`, `product`, `rev`, `rposition`, `skip`, `skip_while`, `step_by`, `sum`, `take`, `take_while`, `to_vec`, `zip`

## Line

`style`, `width`

## Map

`as_array`, `as_array_mut`, `as_object`, `as_object_mut`, `clone`, `contains`, `contains_key`, `count`, `difference`, `drain`, `entry`, `get`, `get_mut`, `insert`, `intersection`, `into_iter`, `into_keys`, `into_values`, `is_disjoint`, `is_empty`, `is_subset`, `is_superset`, `iter`, `keys`, `len`, `remove`, `symmetric_difference`, `union`, `values`, `values_mut`

## Match

`as_str`, `end`, `start`

## Metadata

`accessed`, `created`, `dev`, `gid`, `ino`, `is_dir`, `is_file`, `is_symlink`, `len`, `mode`, `modified`, `mtime`, `permissions`, `readonly`, `uid`

## Modifier

`bits`, `contains`, `difference`, `intersects`, `is_empty`, `union`

## native handles (files, sockets, readers, processes)

`accept`, `by_ref`, `close`, `collect`, `connect`, `duration_since`, `elapsed`, `flush`, `id`, `incoming`, `inner`, `is_cancelled`, `is_panic`, `is_terminal`, `kill`, `kind`, `lines`, `local_addr`, `lock`, `metadata`, `next`, `pad`, `path`, `peer_addr`, `raw_os_error`, `read`, `read_line`, `read_to_end`, `read_to_string`, `read_until`, `seek`, `send`, `send_to`, `set_broadcast`, `set_len`, `set_modified`, `shutdown`, `stderr`, `stdin`, `sync_all`, `sync_data`, `try_clone`, `try_wait`, `wait`, `wait_with_output`, `write`, `write_all`, `write_fmt`, `write_str`

## OpenOptions

`append`, `create`, `create_new`, `open`, `read`, `truncate`, `write`

## Option

`and`, `and_then`, `as_deref`, `as_mut`, `as_ref`, `clone`, `cloned`, `context`, `copied`, `expect`, `filter`, `get`, `into_iter`, `is_none`, `is_none_or`, `is_some`, `is_some_and`, `iter`, `map`, `map_or`, `map_or_else`, `ok_or`, `ok_or_else`, `or`, `or_else`, `take`, `unwrap`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `with_context`, `xor`, `zip`

## Ordering

`is_eq`, `is_ge`, `is_gt`, `is_le`, `is_lt`, `is_ne`, `reverse`, `then`

## OsString

`into`, `is_empty`, `to_str`, `to_string_lossy`

## Output

`status`, `stderr`, `stdout`

## Path

`ancestors`, `as_os_str`, `as_path`, `clone`, `display`, `ends_with`, `exists`, `extension`, `file_name`, `file_stem`, `into_os_string`, `into_string`, `is_absolute`, `is_dir`, `is_file`, `join`, `parent`, `push`, `starts_with`, `to_owned`, `to_path_buf`, `to_str`, `to_string_lossy`, `with_extension`

## RegKey

`create_subkey`, `delete_subkey`, `delete_subkey_all`, `delete_value`, `enum_keys`, `enum_values`, `flags`, `get_raw_value`, `get_value`, `open_subkey`, `open_subkey_with_flags`, `path`, `root`, `set_raw_value`, `set_value`

## Regex

`as_str`, `captures`, `captures_iter`, `find`, `find_iter`, `is_match`, `replace`, `replace_all`, `split`

## Request

`basic_auth`, `bearer_auth`, `body`, `client`, `header`, `json`, `query`, `send`, `timeout`

## Response

`body`, `code`, `content_length`, `error_for_status`, `headers`, `json`, `map`, `status`, `text`

## Result

`and`, `and_then`, `as_deref`, `as_deref_mut`, `as_mut`, `as_ref`, `clone`, `context`, `err`, `expect`, `into_iter`, `is_err`, `is_err_and`, `is_ok`, `is_ok_and`, `iter`, `map`, `map_err`, `map_or`, `map_or_else`, `ok`, `or`, `unwrap`, `unwrap_err`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `with_context`

## Rng

`fill`, `fill_bytes`, `gen`, `gen_bool`, `gen_range`, `random`, `random_bool`, `random_range`

## Row

`height`, `style`

## Service

`account_name`, `change_config`, `current_state`, `dependencies`, `display_name`, `error_control`, `executable_path`, `query_config`, `query_status`, `service_type`, `start`, `start_type`, `stop`

## ServiceManager

`access`, `manager_access`, `name`, `open_service`

## Sha256

`chain_update`, `finalize`, `update`

## Signature

`to_bytes`, `to_vec`

## SigningKey

`as_bytes`, `sign`, `to_bytes`, `verifying_key`

## Span

`content`, `style`, `width`

## Sparkline

`data`, `max`, `style`

## Status

`as_int`, `as_u16`, `is_client_error`, `is_server_error`, `is_success`

## String and str

`as_bytes`, `as_str`, `as_string`, `black`, `blue`, `bold`, `bright_blue`, `bright_cyan`, `bright_green`, `bright_red`, `bright_yellow`, `bytes`, `char_indices`, `chars`, `clear`, `clone`, `cmp`, `contains`, `context`, `count`, `cyan`, `dimmed`, `encode_utf16`, `ends_with`, `eq_ignore_ascii_case`, `expect`, `find`, `get`, `green`, `into_bytes`, `into_owned`, `into_string`, `is_ascii`, `is_char_boundary`, `is_empty`, `is_none`, `is_some`, `italic`, `len`, `lines`, `magenta`, `matches`, `normal`, `on_blue`, `on_green`, `on_red`, `parse`, `purple`, `push`, `push_str`, `red`, `repeat`, `replace`, `replacen`, `reversed`, `rfind`, `rsplit`, `rsplit_once`, `rsplitn`, `split`, `split_once`, `split_whitespace`, `splitn`, `starts_with`, `strip_prefix`, `strip_suffix`, `to_ascii_lowercase`, `to_ascii_uppercase`, `to_lowercase`, `to_owned`, `to_string`, `to_uppercase`, `trim`, `trim_end`, `trim_end_matches`, `trim_matches`, `trim_start`, `trim_start_matches`, `trim_string`, `underline`, `unwrap`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `white`, `with_context`, `yellow`

## Style

`add_modifier`, `bg`, `bold`, `dim`, `fg`, `italic`, `remove_modifier`, `reversed`, `sub_modifier`, `underlined`

## Table

`block`, `column_spacing`, `style`, `widths`

## Vec

`all`, `any`, `append`, `as_array`, `as_array_mut`, `as_object`, `as_object_mut`, `as_slice`, `back`, `back_mut`, `binary_search`, `by_ref`, `chunks`, `clear`, `clone`, `cloned`, `collect`, `collect_map`, `collect_set`, `collect_string`, `concat`, `contains`, `copied`, `copy_from_slice`, `count`, `dedup`, `enumerate`, `extend`, `extend_from_slice`, `filter`, `filter_map`, `find`, `find_map`, `first`, `first_mut`, `flat_map`, `flatten`, `fold`, `for_each`, `front`, `front_mut`, `get`, `get_mut`, `insert`, `into_iter`, `is_empty`, `iter`, `iter_mut`, `join`, `last`, `last_mut`, `len`, `make_contiguous`, `map`, `max`, `max_by_key`, `min`, `min_by_key`, `next`, `next_back`, `nth`, `partition`, `peekable`, `pop`, `pop_back`, `pop_front`, `position`, `product`, `push`, `push_back`, `push_front`, `reduce`, `remove`, `repeat`, `retain`, `rev`, `reverse`, `skip`, `skip_while`, `sort`, `sort_by`, `sort_by_cached_key`, `sort_by_key`, `sort_unstable`, `split_first`, `sum`, `swap`, `swap_remove`, `take`, `take_while`, `to_vec`, `truncate`, `windows`

## VerifyingKey

`as_bytes`, `to_bytes`, `verify`, `verify_strict`

## WmiConnection

`namespace`, `query`, `raw_query`

## builtin (dispatched by id on matching receivers)

`clear`, `clone`, `clone_from`, `cloned`, `cmp`, `contains_key`, `copied`, `get`, `insert`, `make_ascii_lowercase`, `make_ascii_uppercase`, `push`, `push_str`, `replace`, `take`, `to_string`, `unwrap`, `unwrap_or`, `write_all`, `write_fmt`, `write_str`
