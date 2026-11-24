import waywallen.switcher;
int main(int argc, char** argv) {
    rstd::env::args_init(argc, argv);
    return waywallen::switch_cli::run();
}
