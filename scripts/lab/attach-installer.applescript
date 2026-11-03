-- Attach verified removable media after an exported VM is opened in place.
-- No disks are created, erased, installed, or formatted by this operation.
on run argv
    if (count of argv) is not 2 then error "Expected VM UUID and verified installer ISO"
    set guestID to item 1 of argv
    set installer to POSIX file (item 2 of argv)
    get info for installer
    tell application "UTM"
        set guest to virtual machine id guestID
        if status of guest is not stopped then error "Stop the VM before attaching installation media"
        set cfg to configuration of guest
        set diskConfigs to drives of cfg
        if removable of item 1 of diskConfigs is not true then error "First drive is not removable media"
        set diskID to id of item 1 of diskConfigs
        set item 1 of diskConfigs to {id:diskID, source:installer}
        set drives of cfg to diskConfigs
        update configuration of guest with cfg
    end tell
end run
