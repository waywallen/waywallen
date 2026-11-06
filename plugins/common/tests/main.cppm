module;

#include <unistd.h>

export module waywallen.plugin.tests;

import rstd;
import rstd.test;

int main(int argc, char** argv) {
    ::alarm(15);
    rstd::env::args_init(argc, argv);
    return rstd::test::run_registered().to_primitive();
}
