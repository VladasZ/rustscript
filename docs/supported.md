# Supported interpreter surface

Generated from the bridge dispatch tables. Do not edit by hand; run
`rust supported md > docs/supported.md` after changing a bridge, and
the `supported_page_is_current` test enforces it.

A method marked `fast` runs only on the single threaded engine. One
marked `tokio` runs only on the parallel engine that `#[tokio::main]`
selects. Unmarked methods run on both.

## any value

`abs`, `as_array`, `as_bool`, `as_f64`, `as_i128`, `as_i64`, `as_object`, `as_str`, `as_u64`, `as_usize`, `ceil`, `clamp`, `clone`, `cmp`, `floor`, `into` (fast), `is_array` (fast), `is_boolean` (fast), `is_f64` (fast), `is_i64` (fast), `is_multiple_of`, `is_null` (fast), `is_number` (fast), `is_object` (fast), `is_sign_positive`, `is_string` (fast), `is_u64` (fast), `max`, `min`, `mode` (fast), `partial_cmp`, `pow`, `powf`, `powi`, `readonly` (fast), `round`, `saturating_add`, `saturating_mul`, `saturating_sub`, `set_readonly` (fast), `sqrt`, `then_some` (fast), `to_string`, `trunc`

## Base64

`decode` (fast), `encode` (fast), `kind` (fast), `standard_no_pad` (fast), `url_safe` (fast), `url_safe_no_pad` (fast)

## Builder

`build`, `cookie_store`, `redirect` (fast), `timeout`, `user_agent`

## Captures

`get`, `len`, `name`

## Char

`is_alphabetic`, `is_alphanumeric`, `is_ascii`, `is_ascii_alphabetic`, `is_ascii_alphanumeric`, `is_ascii_digit`, `is_ascii_hexdigit`, `is_ascii_lowercase`, `is_ascii_punctuation`, `is_ascii_uppercase`, `is_ascii_whitespace`, `is_lowercase`, `is_numeric`, `is_uppercase`, `is_whitespace`, `to_ascii_lowercase`, `to_ascii_uppercase`, `to_lowercase`, `to_uppercase`

## Child

`id` (tokio), `kill` (tokio), `stderr` (tokio), `stdin` (tokio), `stdout` (tokio), `wait` (tokio), `wait_with_output` (tokio)

## Client

`delete` (tokio), `get` (tokio), `head` (tokio), `patch` (tokio), `post` (tokio), `put` (tokio)

## Command

`arg`, `args`, `current_dir`, `cwd` (fast), `env`, `env_remove`, `output`, `spawn`, `status`, `stderr`, `stdin`, `stdout`

## DateTime

`day`, `format`, `hour`, `minute`, `month`, `second`, `timestamp`, `timestamp_millis`, `to_rfc3339`, `year`

## DirEntry

`file_name` (fast), `file_type` (fast), `name` (fast), `path` (fast)

## Document

`change_page_content` (fast), `get_page_content` (fast), `get_pages` (fast), `save` (fast)

## Duration

`as_micros`, `as_millis`, `as_nanos`, `as_secs`, `as_secs_f64`, `is_zero`, `subsec_micros`, `subsec_millis`, `subsec_nanos`

## Element

`get_text` (fast), `write` (fast)

## Entry

`and_modify` (fast), `key` (fast), `map` (fast), `or_default` (fast), `or_insert` (fast), `or_insert_with` (fast), `or_insert_with_key` (fast)

## Option and Result (tokio mode)

`as_deref` (tokio), `as_mut` (tokio), `as_ref` (tokio), `cloned` (tokio), `context` (tokio), `copied` (tokio), `expect` (tokio), `is_err` (tokio), `is_none` (tokio), `is_ok` (tokio), `is_some` (tokio), `ok` (tokio), `take` (tokio), `unwrap` (tokio), `unwrap_or` (tokio), `unwrap_or_default` (tokio), `with_context` (tokio)

## ExitStatus

`code`, `success`

## FileType

`is_dir` (fast), `is_file` (fast), `is_symlink` (fast)

## HeaderMap

`get`, `get_all` (fast), `map`, `text`

## HeaderValue

`as_str`, `as_string`, `to_str`, `to_string`

## Iterator

`all` (fast), `any` (fast), `as_str` (fast), `cloned` (fast), `collect` (fast), `collect_string` (fast), `copied` (fast), `filter` (fast), `filter_map` (fast), `find` (fast), `flat_map` (fast), `fold` (fast), `for_each` (fast), `last` (fast), `map` (fast), `max` (fast), `max_by_key` (fast), `min` (fast), `min_by_key` (fast), `next` (fast), `partition` (fast), `peek` (fast), `peekable` (fast), `position` (fast), `reduce` (fast), `rev` (fast), `skip_while` (fast), `take_while` (fast), `to_vec` (fast)

## Map

`as_array` (fast), `as_object` (fast), `contains_key` (tokio), `drain` (fast), `get` (tokio), `insert` (tokio), `is_empty` (tokio), `key` (fast), `keys` (tokio), `len` (tokio), `map` (fast), `remove` (tokio), `values` (tokio), `values_mut` (fast)

## Match

`as_str`, `end`, `start`

## Metadata

`accessed` (fast), `created` (fast), `dev` (fast), `gid` (fast), `ino` (fast), `is_dir` (fast), `is_file` (fast), `is_symlink` (fast), `len` (fast), `mode` (fast), `modified` (fast), `mtime` (fast), `permissions` (fast), `readonly` (fast), `uid` (fast)

## native handles (files, sockets, readers, processes)

`accept` (fast), `by_ref` (fast), `close` (fast), `collect` (fast), `connect` (fast), `delete` (fast), `duration_since` (fast), `elapsed` (fast), `flush`, `get` (fast), `head` (fast), `id` (fast), `incoming` (fast), `inner` (fast), `is_terminal` (fast), `kill` (fast), `kind` (fast), `lines`, `local_addr` (fast), `lock` (fast), `metadata` (fast), `next`, `patch` (fast), `path` (fast), `peer_addr` (fast), `post` (fast), `put` (fast), `read` (fast), `read_line`, `read_to_end` (fast), `read_to_string`, `seek` (fast), `send` (fast), `send_to` (fast), `set_broadcast` (fast), `set_len` (fast), `set_modified` (fast), `shutdown` (fast), `stderr` (fast), `stdin` (fast), `sync_all` (fast), `sync_data` (fast), `try_clone` (fast), `try_wait` (fast), `wait` (fast), `wait_with_output` (fast), `write`, `write_all`

## OpenOptions

`append` (fast), `create` (fast), `create_new` (fast), `open` (fast), `read` (fast), `truncate` (fast), `write` (fast)

## Option

`and_then` (fast), `as_deref` (fast), `as_mut` (fast), `as_ref` (fast), `context` (fast), `expect` (fast), `filter` (fast), `get` (fast), `is_none` (fast), `is_some` (fast), `is_some_and` (fast), `map` (fast), `map_or` (fast), `map_or_else` (fast), `ok_or` (fast), `ok_or_else` (fast), `or` (fast), `or_else` (fast), `take` (fast), `unwrap_or_default` (fast), `unwrap_or_else` (fast), `with_context` (fast)

## OsString

`into` (fast), `is_empty` (fast), `to_str` (fast), `to_string_lossy` (fast)

## Output

`status` (tokio), `stderr` (tokio), `stdout` (tokio)

## Path

`ancestors` (fast), `as_os_str` (fast), `as_path` (fast), `clone` (fast), `display` (fast), `exists` (fast), `extension` (fast), `file_name` (fast), `file_stem` (fast), `into_os_string` (fast), `into_string` (fast), `is_absolute` (fast), `is_dir` (fast), `is_file` (fast), `join` (fast), `parent` (fast), `push` (fast), `to_owned` (fast), `to_path_buf` (fast), `to_str` (fast), `to_string_lossy` (fast), `with_extension` (fast)

## RegKey

`create_subkey` (fast), `delete_subkey` (fast), `delete_subkey_all` (fast), `delete_value` (fast), `enum_keys` (fast), `enum_values` (fast), `flags` (fast), `get_raw_value` (fast), `get_value` (fast), `open_subkey` (fast), `open_subkey_with_flags` (fast), `path` (fast), `root` (fast), `set_raw_value` (fast), `set_value` (fast)

## Regex

`as_str`, `captures`, `captures_iter`, `find`, `find_iter`, `is_match`, `replace`, `replace_all`, `split`

## Request

`basic_auth`, `bearer_auth`, `body`, `header`, `headers` (fast), `json`, `query`, `send`, `timeout`

## Response

`body`, `code`, `error_for_status`, `headers`, `json`, `map`, `status`, `text`

## Result

`and_then` (fast), `as_deref` (fast), `as_deref_mut` (fast), `as_mut` (fast), `as_ref` (fast), `clone` (fast), `context` (fast), `err` (fast), `expect` (fast), `is_err` (fast), `is_err_and` (fast), `is_ok` (fast), `is_ok_and` (fast), `map` (fast), `map_err` (fast), `ok` (fast), `unwrap` (fast), `unwrap_err` (fast), `unwrap_or` (fast), `unwrap_or_default` (fast), `unwrap_or_else` (fast), `with_context` (fast)

## Rng

`fill` (fast), `fill_bytes` (fast), `gen` (fast), `gen_bool` (fast), `gen_range` (fast), `random` (fast), `random_bool` (fast), `random_range` (fast)

## Service

`account_name` (fast), `change_config` (fast), `current_state` (fast), `dependencies` (fast), `display_name` (fast), `error_control` (fast), `executable_path` (fast), `query_config` (fast), `query_status` (fast), `service_type` (fast), `start` (fast), `start_type` (fast), `stop` (fast)

## ServiceManager

`access` (fast), `manager_access` (fast), `name` (fast), `open_service` (fast)

## Sha256

`chain_update` (fast), `finalize` (fast), `update` (fast)

## Status

`as_int`, `as_u16`, `is_client_error`, `is_server_error`, `is_success`

## String and str

`as_bytes`, `as_str`, `as_string`, `black`, `blue`, `bold`, `bright_blue`, `bright_cyan`, `bright_green`, `bright_red`, `bright_yellow`, `bytes`, `char_indices`, `chars` (tokio), `clear`, `cmp`, `contains`, `context`, `count`, `cyan`, `dimmed`, `encode_utf16`, `ends_with`, `eq_ignore_ascii_case`, `expect`, `find`, `green`, `into_bytes`, `into_owned`, `into_string`, `is_empty`, `is_none`, `is_some`, `italic`, `len`, `lines` (tokio), `magenta`, `matches`, `normal`, `on_blue`, `on_green`, `on_red`, `parse`, `purple`, `red`, `repeat`, `replace`, `replacen`, `reversed`, `rfind`, `rsplit`, `rsplit_once`, `rsplitn`, `split`, `split_once`, `split_whitespace` (tokio), `splitn`, `starts_with`, `strip_prefix`, `strip_suffix`, `to_ascii_lowercase`, `to_ascii_uppercase`, `to_lowercase`, `to_owned`, `to_uppercase`, `trim`, `trim_end`, `trim_end_matches`, `trim_matches`, `trim_start`, `trim_start_matches`, `trim_string`, `underline`, `unwrap`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `white`, `with_context`, `yellow`

## Vec

`all`, `any`, `append` (fast), `as_array` (fast), `as_object` (fast), `clear`, `cloned` (fast), `collect`, `collect_string`, `concat` (tokio), `contains` (tokio), `copied`, `copy_from_slice`, `count` (tokio), `dedup` (fast), `enumerate` (tokio), `extend`, `extend_from_slice`, `filter`, `filter_map`, `find`, `first` (tokio), `flat_map`, `flatten` (fast), `fold` (fast), `for_each`, `into_iter` (tokio), `is_empty` (tokio), `iter` (tokio), `iter_mut` (tokio), `join` (tokio), `last` (tokio), `len` (tokio), `map`, `max`, `max_by_key` (fast), `min`, `min_by_key` (fast), `next` (fast), `nth`, `partition` (fast), `pop` (tokio), `position`, `product` (tokio), `push` (tokio), `reduce` (fast), `retain` (fast), `rev` (tokio), `reverse` (fast), `skip_while` (fast), `sort` (tokio), `sort_by` (fast), `sort_by_cached_key` (fast), `sort_by_key` (fast), `sum` (tokio), `take_while` (fast), `to_vec`, `truncate` (fast)

## WmiConnection

`namespace` (fast), `query` (fast), `raw_query` (fast)

## builtin (dispatched by id on matching receivers)

`all`, `and_modify`, `and_then`, `any`, `chars`, `clone`, `clone_from`, `cloned`, `concat`, `contains`, `contains_key`, `copied`, `count`, `ends_with`, `entry`, `enumerate`, `filter`, `filter_map`, `find`, `first`, `flat_map`, `fold`, `for_each`, `get`, `get_mut`, `insert`, `into_iter`, `is_empty`, `iter`, `iter_mut`, `join`, `keys`, `last`, `len`, `lines`, `map`, `map_err`, `map_or`, `max_by_key`, `min_by_key`, `ok_or_else`, `or_insert_with`, `or_insert_with_key`, `parse`, `partition`, `pop`, `position`, `product`, `push`, `push_str`, `reduce`, `remove`, `retain`, `rev`, `skip`, `skip_while`, `sort`, `sort_by`, `sort_by_cached_key`, `sort_by_key`, `split`, `split_whitespace`, `starts_with`, `sum`, `take`, `take_while`, `then`, `to_string`, `trim`, `unwrap`, `unwrap_or`, `unwrap_or_else`, `values`, `with_context`
