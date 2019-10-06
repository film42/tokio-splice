Tokio Splice
============

#### About

This is an attempt to implement the `tokio::io::copy` function that lives on `AsyncReadExt` and implement it using the much more efficient splice syscall. There's a lot more work that needs to be done here but the current state works by requiring the reader and writer to implement the `AsRawFd` trait.

#### Results

I piped `/dev/urandom` into a file to generate a 681MB test file. I then used this file to test out the `copy` function in this lib, comparing it to the built-in tokio performance, and also the golang copy implementation which internally uses splice where possible.


Results: Tokio splice copy: 1,439ms, Tokio built-in copy: 65,775ms, and Golang copy: 1,293ms.
