# AlbumFS demo shot list

Target a 20 to 30 second finished recording. The terminal portion should match `docs/demo.tape`, followed by a short visual payoff.

## Terminal sequence

1. Start with a clean 1200 by 700 terminal, a readable 22 px font, and the `album`, `mount`, and current directory already prepared.
2. Format the photo album with an explicit passphrase and anchor:

   ```console
   albumfs format --passphrase demo-passphrase --anchor album/anchor.jpg album
   ```

3. Mount the filesystem:

   ```console
   albumfs mount --passphrase demo-passphrase --anchor album/anchor.jpg album mount &
   ```

4. Create a directory and write a recognizable note:

   ```console
   mkdir mount/letters
   printf 'Meet me by the old lighthouse.\n' > mount/letters/note.txt
   ```

5. Read the note back, leave the result on screen briefly, then unmount:

   ```console
   cat mount/letters/note.txt
   albumfs umount mount
   ```

## Photo payoff

After unmounting, open one of the carrier JPEGs in a normal image viewer or file manager. Hold on the ordinary-looking photo for three to five seconds. This beat needs a real screen capture or split screen because a terminal recorder cannot capture it. A clean option is to record the terminal with `vhs`, record the photo opening separately, and composite the photo shot immediately after the unmount command.

## Recording notes

- Keep the terminal at 1200 by 700 and use a 20 to 22 px font so commands remain legible after compression.
- Aim for about 20 seconds of terminal footage and no more than 30 seconds overall.
- Use unhurried pauses after format, mount, `cat`, and unmount.
- Put familiar photos in the album. The file written into the mount should also be recognizable, such as a short personal note or a small document, so the viewer feels what is being hidden.
- Prepare a disposable copy of the album before recording because format and filesystem writes intentionally modify the carriers.
