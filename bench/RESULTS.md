# Benchmark results

Every case in the suite, one chart each, in run order. Each bar is the
median of that case's samples. [README.md](README.md) explains the method and
what every case measures.

This file is written by `cargo run --release --bin chart` together with the
charts themselves. Edit that tool, not this file.

- machine: Apple M1 Pro, 10 cores, macos aarch64
- runtimes: node v26.7.0, Python 3.14.7, rustc 1.96.1
- run: commit `86d2572`, dirty tree, 1 warmup, 5 total samples, 5 compute samples

## hello world

`hello`, startup

![hello world](results/hello.png)

## big script startup

`big_script`, startup

![big script startup](results/big_script.png)

## multi-file startup

`multifile_startup`, startup

![multi-file startup](results/multifile_startup.png)

## recursive fibonacci

`fib`, compute, `size=27`

![recursive fibonacci](results/fib.png)

## sieve of eratosthenes

`sieve`, compute, `size=250000`

![sieve of eratosthenes](results/sieve.png)

## mandelbrot

`mandelbrot`, compute, `size=140`

![mandelbrot](results/mandelbrot.png)

## collatz

`collatz`, compute, `size=10000`

![collatz](results/collatz.png)

## binary trees

`binary_trees`, compute, `size=11`

![binary trees](results/binary_trees.png)

## string building

`string_builder`, compute, `size=200000`

![string building](results/string_builder.png)

## map filter fold

`higher_order`, compute, `size=100000`

![map filter fold](results/higher_order.png)

## comparator sort

`sort`, compute, `size=50000`

![comparator sort](results/sort.png)

## sort by key

`sort_key`, compute, `size=50000`

![sort by key](results/sort_key.png)

## int hashmap

`hashmap_int`, compute, `size=150000`

![int hashmap](results/hashmap_int.png)

## n-body

`nbody`, compute, `size=8000`

![n-body](results/nbody.png)

## json serialize

`json_serialize`, compute, `size=100000`

![json serialize](results/json_serialize.png)

## stdout lines

`stdout_lines`, compute, `size=20000`

![stdout lines](results/stdout_lines.png)

## word count

`word_count`, compute, `fixture=word_count/data.txt`

![word count](results/word_count.png)

## json parse

`json`, compute, `fixture=json/data.json`

![json parse](results/json.png)

## regex

`regex`, compute, `fixture=word_count/data.txt`

![regex](results/regex.png)

## file transform

`file_transform`, compute, `fixture=word_count/data.txt`

![file transform](results/file_transform.png)

## process spawn

`process_spawn`, compute, `helper_runs=20`

![process spawn](results/process_spawn.png)

## async tasks

`async_tasks`, compute, `size=20`

![async tasks](results/async_tasks.png)

## local http

`http_local`, compute, `requests=100`

![local http](results/http_local.png)

## automation script

`automation`, compute, `fixture=word_count/data.txt`, `top=20`

![automation script](results/automation.png)

