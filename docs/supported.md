# Supported interpreter surface

Generated from the bridge dispatch tables. Do not edit by hand; run
`rust supported md > docs/supported.md` after changing a bridge, and
the `supported_page_is_current` test enforces it.

## any value

`abs`, `as_array`, `as_array_mut`, `as_bool`, `as_f64`, `as_i128`, `as_i64`, `as_object`, `as_object_mut`, `as_str`, `as_u64`, `as_usize`, `ceil`, `clamp`, `clone`, `cmp`, `extend`, `extend_from_slice`, `floor`, `fract`, `get`, `into`, `is_array`, `is_boolean`, `is_f64`, `is_finite`, `is_i64`, `is_infinite`, `is_multiple_of`, `is_nan`, `is_null`, `is_number`, `is_object`, `is_sign_negative`, `is_sign_positive`, `is_string`, `is_u64`, `max`, `min`, `mode`, `mul_add`, `partial_cmp`, `pointer`, `pointer_mut`, `pow`, `powf`, `powi`, `readonly`, `recip`, `round`, `saturating_add`, `saturating_mul`, `saturating_sub`, `set_readonly`, `signum`, `sqrt`, `then`, `then_some`, `to_string`, `trunc`

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

`style`

## Char

`is_alphabetic`, `is_alphanumeric`, `is_ascii`, `is_ascii_alphabetic`, `is_ascii_alphanumeric`, `is_ascii_digit`, `is_ascii_hexdigit`, `is_ascii_lowercase`, `is_ascii_punctuation`, `is_ascii_uppercase`, `is_ascii_whitespace`, `is_lowercase`, `is_numeric`, `is_uppercase`, `is_whitespace`, `to_ascii_lowercase`, `to_ascii_uppercase`, `to_digit`, `to_lowercase`, `to_uppercase`

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

`and_modify`, `key`, `map`, `or_default`, `or_insert`, `or_insert_with`, `or_insert_with_key`

## ExitStatus

`code`, `success`

## FileType

`is_dir`, `is_file`, `is_symlink`

## HeaderMap

`get`, `map`, `text`

## HeaderValue

`as_str`, `as_string`, `to_str`, `to_string`

## Iterator

`all`, `any`, `as_str`, `by_ref`, `cloned`, `collect`, `collect_map`, `collect_set`, `collect_string`, `copied`, `filter`, `filter_map`, `find`, `for_each`, `last`, `map`, `max`, `min`, `next`, `peek`, `peekable`, `position`, `rev`, `skip_while`, `take_while`, `to_vec`

## Line

`style`, `width`

## Map

`as_array`, `as_array_mut`, `as_object`, `as_object_mut`, `drain`, `key`, `map`, `values_mut`

## Match

`as_str`, `end`, `start`

## Metadata

`accessed`, `created`, `dev`, `gid`, `ino`, `is_dir`, `is_file`, `is_symlink`, `len`, `mode`, `modified`, `mtime`, `permissions`, `readonly`, `uid`

## Modifier

`bits`, `contains`, `difference`, `intersects`, `is_empty`, `union`

## native handles (files, sockets, readers, processes)

`accept`, `by_ref`, `close`, `collect`, `connect`, `duration_since`, `elapsed`, `flush`, `id`, `incoming`, `inner`, `is_terminal`, `kill`, `kind`, `lines`, `local_addr`, `lock`, `metadata`, `next`, `path`, `peer_addr`, `read`, `read_line`, `read_to_end`, `read_to_string`, `read_until`, `seek`, `send`, `send_to`, `set_broadcast`, `set_len`, `set_modified`, `shutdown`, `stderr`, `stdin`, `sync_all`, `sync_data`, `try_clone`, `try_wait`, `wait`, `wait_with_output`, `write`, `write_all`

## OpenOptions

`append`, `create`, `create_new`, `open`, `read`, `truncate`, `write`

## Option

`and_then`, `as_deref`, `as_mut`, `as_ref`, `context`, `expect`, `filter`, `get`, `into_iter`, `is_none`, `is_some`, `is_some_and`, `iter`, `map`, `map_or`, `map_or_else`, `ok_or`, `ok_or_else`, `or`, `or_else`, `take`, `unwrap_or_default`, `unwrap_or_else`, `with_context`

## OsString

`into`, `is_empty`, `to_str`, `to_string_lossy`

## Output

`status`, `stderr`, `stdout`

## Path

`ancestors`, `as_os_str`, `as_path`, `clone`, `display`, `exists`, `extension`, `file_name`, `file_stem`, `into_os_string`, `into_string`, `is_absolute`, `is_dir`, `is_file`, `join`, `parent`, `push`, `to_owned`, `to_path_buf`, `to_str`, `to_string_lossy`, `with_extension`

## RegKey

`create_subkey`, `delete_subkey`, `delete_subkey_all`, `delete_value`, `enum_keys`, `enum_values`, `flags`, `get_raw_value`, `get_value`, `open_subkey`, `open_subkey_with_flags`, `path`, `root`, `set_raw_value`, `set_value`

## Regex

`as_str`, `captures`, `captures_iter`, `find`, `find_iter`, `is_match`, `replace`, `replace_all`, `split`

## Request

`basic_auth`, `bearer_auth`, `body`, `client`, `header`, `json`, `query`, `send`, `timeout`

## Response

`body`, `code`, `content_length`, `error_for_status`, `headers`, `json`, `map`, `status`, `text`

## Result

`and_then`, `as_deref`, `as_deref_mut`, `as_mut`, `as_ref`, `clone`, `context`, `err`, `expect`, `into_iter`, `is_err`, `is_err_and`, `is_ok`, `is_ok_and`, `iter`, `map`, `map_err`, `map_or`, `map_or_else`, `ok`, `unwrap`, `unwrap_err`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `with_context`

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

## Span

`content`, `style`, `width`

## Sparkline

`data`, `max`, `style`

## Status

`as_int`, `as_u16`, `is_client_error`, `is_server_error`, `is_success`

## String and str

`as_bytes`, `as_str`, `as_string`, `black`, `blue`, `bold`, `bright_blue`, `bright_cyan`, `bright_green`, `bright_red`, `bright_yellow`, `bytes`, `char_indices`, `clear`, `cmp`, `contains`, `context`, `count`, `cyan`, `dimmed`, `encode_utf16`, `ends_with`, `eq_ignore_ascii_case`, `expect`, `find`, `green`, `into_bytes`, `into_owned`, `into_string`, `is_empty`, `is_none`, `is_some`, `italic`, `len`, `magenta`, `matches`, `normal`, `on_blue`, `on_green`, `on_red`, `purple`, `red`, `repeat`, `replace`, `replacen`, `reversed`, `rfind`, `rsplit`, `rsplit_once`, `rsplitn`, `split`, `split_once`, `splitn`, `starts_with`, `strip_prefix`, `strip_suffix`, `to_ascii_lowercase`, `to_ascii_uppercase`, `to_lowercase`, `to_owned`, `to_uppercase`, `trim`, `trim_end`, `trim_end_matches`, `trim_matches`, `trim_start`, `trim_start_matches`, `trim_string`, `underline`, `unwrap`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `white`, `with_context`, `yellow`

## Style

`add_modifier`, `bg`, `bold`, `dim`, `fg`, `italic`, `remove_modifier`, `reversed`, `sub_modifier`, `underlined`

## Table

`block`, `column_spacing`, `style`, `widths`

## Vec

`all`, `any`, `append`, `as_array`, `as_array_mut`, `as_object`, `as_object_mut`, `by_ref`, `clear`, `cloned`, `collect`, `collect_map`, `collect_set`, `collect_string`, `copied`, `copy_from_slice`, `dedup`, `extend`, `extend_from_slice`, `filter`, `filter_map`, `find`, `flat_map`, `flatten`, `fold`, `for_each`, `map`, `max`, `max_by_key`, `min`, `min_by_key`, `next`, `nth`, `partition`, `position`, `reduce`, `retain`, `reverse`, `skip_while`, `sort_by`, `sort_by_cached_key`, `sort_by_key`, `swap_remove`, `take_while`, `to_vec`, `truncate`

## WmiConnection

`namespace`, `query`, `raw_query`

## builtin (dispatched by id on matching receivers)

`all`, `and_modify`, `and_then`, `any`, `chars`, `clone`, `clone_from`, `cloned`, `concat`, `contains`, `contains_key`, `copied`, `count`, `ends_with`, `entry`, `enumerate`, `filter`, `filter_map`, `find`, `first`, `flat_map`, `fold`, `for_each`, `get`, `get_mut`, `insert`, `into_iter`, `into_keys`, `into_values`, `is_empty`, `iter`, `iter_mut`, `join`, `keys`, `last`, `len`, `lines`, `map`, `map_err`, `map_or`, `max_by_key`, `min_by_key`, `ok_or_else`, `or_insert_with`, `or_insert_with_key`, `parse`, `partition`, `pop`, `position`, `product`, `push`, `push_str`, `reduce`, `remove`, `retain`, `rev`, `skip`, `skip_while`, `sort`, `sort_by`, `sort_by_cached_key`, `sort_by_key`, `sort_unstable`, `split`, `split_first`, `split_whitespace`, `starts_with`, `sum`, `take`, `take_while`, `then`, `to_string`, `trim`, `unwrap`, `unwrap_or`, `unwrap_or_else`, `values`, `with_context`
