# Tuios

Tuios is a TUI interface for the USOS system, supporting fuzzy search across students, tests.
As of now it only supports view from the perspective of a teacher.
It uses the official USOS API and should work for any University that uses it and allows API access.
On start-up, you should be given instructions on how to connect.

Disclaimer: all the code here has been written by Claude. Because this is just another interface for an external service, there should be no security problems with anything.
But the code might be doing something stupid. Use at your own risk.

![The tree of tasks and grades of a course](docs/screenshots/tree.png)

## Screenshots

All screenshots show `tuios --mock`: made-up students, courses and points.

| | |
|---|---|
| ![Points and comments of every student for one task](docs/screenshots/students.png) | ![Fuzzy search: "kwk" finds Krawczyk and Kowalczyk](docs/screenshots/fuzzy.png) |
| Every student's points and comment for one task | Fuzzy search everywhere |
| ![Entering points and a comment](docs/screenshots/form.png) | ![Choosing a grade on the 2–5 scale](docs/screenshots/grade.png) |
| Points with a comment for the student | Grades, typed or picked with ←→ |
| ![All tasks and grades of one student](docs/screenshots/student.png) | ![First start: getting an API key](docs/screenshots/setup.png) |
| One student's whole record | First start explains how to get an API key |

## Install

You need a Rust toolchain (<https://rustup.rs>). Then, from this directory:

```sh
cargo install --path .
tuios
```

To look around first without a USOS account, run `tuios --mock`. It never connects to
USOS: the students, courses and points are random, made up on each start.
