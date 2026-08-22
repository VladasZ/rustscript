# Benchmark results

One chart per case, in run order. Each bar is the median of that case's
samples. The method is in [README.md](README.md).

This file is written by `cargo run --release --bin chart`. Edit that tool,
not this file.

- machine: Apple M1 Pro, 10 cores, macos aarch64
- runtimes: node v26.7.0, Python 3.14.7, rustc 1.96.1
- run: commit `86d2572`, dirty tree, 1 warmup, 5 total samples, 5 compute samples

## hello world

`hello`, startup

![hello world](results/hello.png)

Scripts: [cases/hello](cases/hello)

## big script startup

`big_script`, startup

![big script startup](results/big_script.png)

Scripts: [cases/big_script](cases/big_script)

## multi-file startup

`multifile_startup`, startup

![multi-file startup](results/multifile_startup.png)

Scripts: [cases/multifile_startup](cases/multifile_startup)

## recursive fibonacci

`fib`, compute, `size=27`

![recursive fibonacci](results/fib.png)

Scripts: [cases/fib](cases/fib)

## sieve of eratosthenes

`sieve`, compute, `size=250000`

![sieve of eratosthenes](results/sieve.png)

Scripts: [cases/sieve](cases/sieve)

## mandelbrot

`mandelbrot`, compute, `size=140`

![mandelbrot](results/mandelbrot.png)

Scripts: [cases/mandelbrot](cases/mandelbrot)

## collatz

`collatz`, compute, `size=10000`

![collatz](results/collatz.png)

Scripts: [cases/collatz](cases/collatz)

## binary trees

`binary_trees`, compute, `size=11`

![binary trees](results/binary_trees.png)

Scripts: [cases/binary_trees](cases/binary_trees)

## string building

`string_builder`, compute, `size=200000`

![string building](results/string_builder.png)

Scripts: [cases/string_builder](cases/string_builder)

## map filter fold

`higher_order`, compute, `size=100000`

![map filter fold](results/higher_order.png)

Scripts: [cases/higher_order](cases/higher_order)

## comparator sort

`sort`, compute, `size=50000`

![comparator sort](results/sort.png)

Scripts: [cases/sort](cases/sort)

## sort by key

`sort_key`, compute, `size=50000`

![sort by key](results/sort_key.png)

Scripts: [cases/sort_key](cases/sort_key)

## int hashmap

`hashmap_int`, compute, `size=150000`

![int hashmap](results/hashmap_int.png)

Scripts: [cases/hashmap_int](cases/hashmap_int)

## n-body

`nbody`, compute, `size=8000`

![n-body](results/nbody.png)

Scripts: [cases/nbody](cases/nbody)

## json serialize

`json_serialize`, compute, `size=100000`

![json serialize](results/json_serialize.png)

Scripts: [cases/json_serialize](cases/json_serialize)

## stdout lines

`stdout_lines`, compute, `size=20000`

![stdout lines](results/stdout_lines.png)

Scripts: [cases/stdout_lines](cases/stdout_lines)

## word count

`word_count`, compute, `fixture=word_count/data.txt`

![word count](results/word_count.png)

Scripts: [cases/word_count](cases/word_count)

## json parse

`json`, compute, `fixture=json/data.json`

![json parse](results/json.png)

Scripts: [cases/json](cases/json)

## regex

`regex`, compute, `fixture=word_count/data.txt`

![regex](results/regex.png)

Scripts: [cases/regex](cases/regex)

## file transform

`file_transform`, compute, `fixture=word_count/data.txt`

![file transform](results/file_transform.png)

Scripts: [cases/file_transform](cases/file_transform)

## process spawn

`process_spawn`, compute, `helper_runs=20`

![process spawn](results/process_spawn.png)

Scripts: [cases/process_spawn](cases/process_spawn)

## async tasks

`async_tasks`, compute, `size=20`

![async tasks](results/async_tasks.png)

Scripts: [cases/async_tasks](cases/async_tasks)

## local http

`http_local`, compute, `requests=100`

![local http](results/http_local.png)

Scripts: [cases/http_local](cases/http_local)

## automation script

`automation`, compute, `fixture=word_count/data.txt`, `top=20`

![automation script](results/automation.png)

Scripts: [cases/automation](cases/automation)

