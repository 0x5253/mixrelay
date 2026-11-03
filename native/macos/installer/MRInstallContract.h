#ifndef MR_INSTALL_CONTRACT_H
#define MR_INSTALL_CONTRACT_H

#define MR_PRODUCT_DIRECTORY "/Applications/MixRelay"
#define MR_APP_BUNDLE_NAME "MixRelay.app"
#define MR_UNINSTALLER_BUNDLE_NAME "Uninstall MixRelay.app"
#define MR_APP_PATH MR_PRODUCT_DIRECTORY "/" MR_APP_BUNDLE_NAME
#define MR_UNINSTALLER_PATH MR_PRODUCT_DIRECTORY "/" MR_UNINSTALLER_BUNDLE_NAME
#define MR_APP_EXECUTABLE_PATH MR_APP_PATH "/Contents/MacOS/mixrelay-desktop"
#define MR_APP_INFO_PATH MR_APP_PATH "/Contents/Info.plist"
#define MR_REMOVAL_HELPER_PATH MR_UNINSTALLER_PATH "/Contents/Helpers/mixrelay-macos-installer"

#endif
