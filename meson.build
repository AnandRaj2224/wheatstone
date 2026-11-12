project('whetstone', 'rust',
          version: '0.15.2',
    meson_version: '>= 1.7.0',
  default_options: [ 'warning_level=2', 'werror=false', ],
)

i18n = import('i18n')
gnome = import('gnome')

subdir('data')
subdir('src')
subdir('po')

# Freedesktop AppData File
appdata_conf = configuration_data()
appdata_conf.set('app-id', application_id)

metainfo_configured = configure_file(
    input: 'data/dev.whetstone.editor.metainfo.xml.in',
    output: 'configured' + '@BASENAME@',
    configuration: appdata_conf
)

gnome.post_install(
     glib_compile_schemas: true,
    gtk_update_icon_cache: true,
  update_desktop_database: true,
     update_mime_database: true,
)
