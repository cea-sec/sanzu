Building Sanzu
==============

The building and packaging process uses Docker containers.

* **Debian bullseye** : `make debian (creates 3 .deb packages)
* **Almalinux 8** : `make almalinux8` 
* **Windows** : `make windows` (client only)

Two more options:
* **clean**:  `make clean` to cleanup the target directory
* **mrproper** : `make prproper` to cleanup and removes all docker images
