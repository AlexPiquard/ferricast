# Resources

fs = import('fs')
blueprint_compiler = find_program('blueprint-compiler')

blueprints = files(
  'ui/shortcuts.blp',
  'ui/window.blp',
  'ui/render.blp',
  'ui/editor.blp',
)

ui_files = []
foreach blp : blueprints
  ui_files += custom_target(
    'compile-@0@'.format(fs.name(blp)),
    input: blp,
    output: fs.name(blp).replace('.blp', '.ui'),
    command: [blueprint_compiler, 'compile', '--output', '@OUTPUT@', '@INPUT@'],
    build_by_default: true,
  )
endforeach

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
  dependencies: [appdata_file] + ui_files,
)
