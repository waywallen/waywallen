export module waywallen.image.tests;
import rstd;
import rstd.test;

int main(int argc, char** argv) {
    rstd::env::args_init(argc, argv);
    return rstd::test::run_registered().to_primitive();
}
