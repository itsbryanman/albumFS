# Run with: vhs docs/demo.tape
# Requires vhs from https://github.com/charmbracelet/vhs

Output docs/demo.gif

Set Shell "bash"
Set FontSize 22
Set Width 1200
Set Height 700
Set TypingSpeed 35ms
Set PlaybackSpeed 1.0

Type "albumfs format --passphrase demo-passphrase --anchor album/anchor.jpg album"
Enter
Sleep 2s

Type "albumfs mount --passphrase demo-passphrase --anchor album/anchor.jpg album mount &"
Enter
Sleep 3s

Type "mkdir mount/letters"
Enter
Sleep 1s

Type "printf 'Meet me by the old lighthouse.\n' > mount/letters/note.txt"
Enter
Sleep 2s

Type "cat mount/letters/note.txt"
Enter
Sleep 3s

Type "albumfs umount mount"
Enter
Sleep 3s
