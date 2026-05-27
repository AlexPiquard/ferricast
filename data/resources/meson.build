# Resources

conf = configuration_data()
conf.set('PATH_ID', path_id)
conf.set('APP_ID', application_id)
gresource_file = configure_file(
  input: '@0@.gresource.xml.in'.format(base_id),
  output: '@0@.gresource.xml'.format(application_id),
  configuration: conf,
)

resources = gnome.compile_resources(
  'resources',
  gresource_file,
  gresource_bundle: true,
  source_dir: meson.current_build_dir(),
  install: true,
  install_dir: pkgdatadir,
  dependencies: appdata_file,
)
