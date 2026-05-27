project(
  'ferricast',
  'rust',
  version: '0.1.0',
  meson_version: '>= 0.59',
license: 'GPL-3.0-or-later',
)

i18n = import('i18n')
gnome = import('gnome')

base_id = 'fr.alexpiquard.ferricast'
path_id = '/fr/alexpiquard/ferricast/'

dependency('gtk4', version: '>= 4.10')
dependency('libadwaita-1', version: '>= 1.8.alpha')
dependency('gstreamer-1.0', version: '>= 1.24')
dependency('gstreamer-video-1.0', version: '>= 1.24')
dependency('gstreamer-app-1.0', version: '>= 1.24')
dependency('gstreamer-controller-1.0', version: '>= 1.24')
dependency('gst-editing-services-1.0', version: '>= 1.24')
dependency('graphene-gobject-1.0', version: '>= 1.10')
dependency('libpipewire-0.3', version: '>= 0.3.32')

find_program('glib-compile-resources')
glib_compile_schemas = find_program('glib-compile-schemas')
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstreamcli = find_program('appstreamcli', required: false)
cargo = find_program('cargo')

name = 'Ferricast'
version = meson.project_version()

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
localedir = prefix / get_option('localedir')

datadir = prefix / get_option('datadir')
pkgdatadir = datadir / meson.project_name()
iconsdir = datadir / 'icons'
podir = meson.project_source_root() / 'po'
gettext_package = meson.project_name()

if get_option('profile') == 'development'
  message('Building in debug mode')

  profile = 'Devel'
  vcs_tag = run_command('git', 'rev-parse', '--short', 'HEAD', check: false).stdout().strip()
  if vcs_tag == ''
    version_suffix = '-devel'
  else
    version_suffix = '-@0@'.format(vcs_tag)
  endif
  application_id = '@0@.@1@'.format(base_id, profile)

  cargo_release_option = []
  rust_target = 'debug'
else
  profile = ''
  version_suffix = ''
  application_id = base_id
  cargo_release_option = [ '--release' ]
  rust_target = 'release'
endif

cargo_home = meson.project_build_root() / 'cargo-home'
cargo_target = meson.project_build_root() / 'cargo-target'
cargo_env = environment()
cargo_env.set('CARGO_HOME', cargo_home)
cargo_env.set('CARGO_TARGET_DIR', cargo_target)
cargo_env.set('MESON_APP_NAME', name)
cargo_env.set('MESON_APP_ID', application_id)
cargo_env.set('MESON_PATH_ID', path_id)
cargo_env.set('MESON_GETTEXT_PACKAGE', gettext_package)
cargo_env.set('MESON_LOCALEDIR', localedir)
cargo_env.set('MESON_PKGDATADIR', pkgdatadir)
cargo_env.set('MESON_PROFILE', profile)
cargo_env.set('MESON_VERSION', version + version_suffix)
cargo_env.set('MESON_RESOURCES_FILE', pkgdatadir / 'resources.gresource')

meson.add_devenv(cargo_env)

# Required if you run dist though
find_program('cargo-vendor-filterer', required: false)
meson.add_dist_script(
  'build-aux/dist-vendor.sh',
  cargo_home,
  cargo_target,
)

if get_option('profile') == 'development'
  # Setup pre-commit hook for ensuring coding style is always consistent
  message('Setting up git pre-commit hook..')
  run_command('cp', '-f', 'hooks/pre-commit.hook', '.git/hooks/pre-commit', check: false)
endif

subdir('data')
subdir('po')
subdir('src')

test (
  'cargo-test',
  cargo,
  args: [
    'test',
    cargo_release_option,
    '--all-targets',
    '--all-features',
    '--workspace',
  ],
  env: cargo_env,
  timeout: 400,
)

gnome.post_install(
  gtk_update_icon_cache: true,
  glib_compile_schemas: true,
  update_desktop_database: true,
)
