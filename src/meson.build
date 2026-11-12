# Extract metadata from Cargo.toml
cargo_toml = meson.project_source_root() / 'Cargo.toml'
python3 = import('python').find_installation('python3')
app_id = run_command(
  python3,
  '-c',
  'import tomllib, sys; print(tomllib.load(open(sys.argv[1], "rb"))["package"]["metadata"]["app_id"])',
  cargo_toml,
  check: true
).stdout().strip()
app_base_path = '/' + app_id.replace('.', '/')

# Extract the exact resolved 'typst' crate version from Cargo.lock, so the
# About dialog can report the actual version linked into the binary rather
# than the semver range from Cargo.toml.
cargo_lock = meson.project_source_root() / 'Cargo.lock'
typst_version = run_command(
  python3,
  '-c',
  'import tomllib, sys; data = tomllib.load(open(sys.argv[1], "rb")); print(next(p["version"] for p in data["package"] if p["name"] == "typst"))',
  cargo_lock,
  check: true
).stdout().strip()

# Set config constants
conf = configuration_data()
conf.set_quoted('APP_BASE_PATH', app_base_path)
conf.set_quoted('APP_ID', app_id)
conf.set_quoted('GETTEXT_PACKAGE', 'whetstone')
conf.set_quoted('LOCALEDIR', get_option('prefix') / get_option('localedir'))
conf.set_quoted('PKGDATADIR', pkgdatadir)
conf.set_quoted('TYPST_VERSION', typst_version)
conf.set_quoted('VERSION', meson.project_version())

configure_file(
    input: 'config.rs.in',
    output: 'config.rs',
    configuration: conf
)

# Copy the config.rs output to the source directory.
run_command(
  'cp',
  meson.project_build_root() / 'src' / 'config.rs',
  meson.project_source_root() / 'src' / 'config.rs',
  check: true
)

cargo_bin  = find_program('cargo')
cargo_opt  = [ '--manifest-path', meson.project_source_root() / 'Cargo.toml' ]
cargo_opt += [ '--target-dir', meson.project_build_root() / 'src' ]

message('Cargo options: @0@'.format(cargo_opt))

cargo_env  = environment()
cargo_env.set('CARGO_HOME', meson.project_build_root() / 'cargo-home')

if get_option('buildtype') == 'release'
  cargo_opt += [ '--release' ]
  rust_target = 'release'
else
  rust_target = 'debug'
endif

cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  env: cargo_env,
  command: [
    cargo_bin, 'build',
    cargo_opt, '&&', 'cp', 'src' / rust_target / meson.project_name(), '@OUTPUT@',
  ]
)

cargo_test = custom_target(
  'cargo-test',
  build_by_default: false,
  build_always_stale: true,
  output: 'cargo-test.stamp',
  console: true,
  env: cargo_env,
  command: [
    cargo_bin, 'test',
    cargo_opt,
  ]
)

cargo_clippy = custom_target(
  'cargo-clippy',
  build_by_default: false,
  build_always_stale: true,
  output: 'cargo-clippy.stamp',
  console: true,
  env: cargo_env,
  command: [
    cargo_bin, 'clippy',
    cargo_opt,
  ]
)