cargo_build_options = [ '--manifest-path', meson.project_source_root() / 'Cargo.toml' ]
cargo_build_options += cargo_release_option

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  depends: resources,
  env: cargo_env,
  command: [
    cargo, 'build',
    cargo_build_options,
    '&&',
    'cp', cargo_target / rust_target / meson.project_name(), '@OUTPUT@',
  ]
)
