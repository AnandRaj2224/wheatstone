i18n.gettext('whetstone', preset: 'glib')

custom_target('pot-update',
  input: 'POTFILES',
  output: 'whetstone.pot',
  command: ['xgettext', '--from-code=UTF-8', '--add-comments',
            '--keyword=_', '--keyword=C_',
            '--directory=@SOURCE_ROOT@',
            '--files-from=@INPUT@',
            '-o', '@SOURCE_ROOT@/po/whetstone.pot'],
  build_by_default: true,
)