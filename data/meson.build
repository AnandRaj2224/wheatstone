desktop_file = i18n.merge_file(
        input: 'dev.whetstone.editor.desktop.in',
       output: 'dev.whetstone.editor.desktop',
         type: 'desktop',
       po_dir: '../po',
      install: true,
  install_dir: get_option('datadir') / 'applications'
)

desktop_utils = find_program('desktop-file-validate', required: false)
if desktop_utils.found()
  test('Validate desktop file', desktop_utils, args: [desktop_file])
endif

appstream_file = i18n.merge_file(
        input: 'dev.whetstone.editor.metainfo.xml.in',
       output: 'dev.whetstone.editor.metainfo.xml',
       po_dir: '../po',
      install: true,
  install_dir: get_option('datadir') / 'metainfo'
)

appstreamcli = find_program('appstreamcli', required: false, disabler: true)
test('Validate appstream file', appstreamcli,
     args: ['validate', '--no-net', '--explain', appstream_file])

install_data('dev.whetstone.editor.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas'
)

compile_schemas = find_program('glib-compile-schemas', required: false, disabler: true)
test('Validate schema file',
     compile_schemas,
     args: ['--strict', '--dry-run', meson.current_source_dir()])

service_conf = configuration_data()
service_conf.set('bindir', get_option('prefix') / get_option('bindir'))
configure_file(
  input: 'dev.whetstone.editor.service.in',
  output: 'dev.whetstone.editor.service',
  configuration: service_conf,
  install_dir: get_option('datadir') / 'dbus-1' / 'services'
)

# Install mime file
install_data(
  'dev.whetstone.editor.mime.xml',
  install_dir: get_option('datadir') / 'mime' / 'packages'
)

# Install fonts
install_data(
  'fonts/Iosevka/TypesetterSlab-Regular.ttf',
  'fonts/Iosevka/TypesetterSlab-Bold.ttf',
  'fonts/Iosevka/TypesetterSlab-Italic.ttf',
  'fonts/Iosevka/TypesetterSlab-BoldItalic.ttf',
  'fonts/Source Code/SourceCodeVF-Italic.ttf',
  'fonts/Source Code/SourceCodeVF-Upright.ttf',
  'fonts/Source Sans/SourceSans3VF-Italic.ttf',
  'fonts/Source Sans/SourceSans3VF-Upright.ttf',
  'fonts/Source Serif/SourceSerif4Variable-Italic.ttf',
  'fonts/Source Serif/SourceSerif4Variable-Roman.ttf',
  install_dir: get_option('datadir') / 'fonts'
)

subdir('icons')

# Compile gresources
pkgdatadir = get_option('prefix') / get_option('datadir') / meson.project_name()
gnome.compile_resources('whetstone',
  'whetstone.gresource.xml',
  source_dir: meson.current_source_dir(),
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
)

install_subdir(
  'templates',
  install_dir: pkgdatadir,
)