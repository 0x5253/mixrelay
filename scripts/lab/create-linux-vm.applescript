-- UTM's documented scripting API creates in its internal staging directory.
-- Export an unbooted copy to the selected external volume. The caller must
-- validate that copy before removing the staging VM and opening the external
-- bundle in place; UTM's `import` command would copy it back to the internal disk.
-- Arguments are data, never interpolated AppleScript source.
on run argv
    if (count of argv) is not 4 then error "Expected name, architecture, ISO path, destination .utm path"
    set guestName to item 1 of argv
    set guestArchitecture to item 2 of argv
    if guestArchitecture is not "aarch64" and guestArchitecture is not "x86_64" then error "Unsupported architecture"
    set installer to POSIX file (item 3 of argv)
    set destination to POSIX file (item 4 of argv)
    get info for installer
    set destinationExists to false
    try
        get info for destination
        set destinationExists to true
    end try
    if destinationExists then error "Destination exists; refusing to overwrite it"
    set useHypervisor to guestArchitecture is "aarch64"
    tell application "UTM"
        if exists (virtual machine named guestName) then error "VM already exists; refusing to replace it"
        set guest to make new virtual machine with properties {backend:qemu, configuration:{name:guestName, architecture:guestArchitecture, memory:6144, cpu cores:4, hypervisor:useHypervisor, uefi:true, drives:{{removable:true, source:installer}, {guest size:65536}}, displays:{{hardware:"virtio-gpu-pci", dynamic resolution:true}}}}
        export guest to destination
        return id of guest
    end tell
end run
