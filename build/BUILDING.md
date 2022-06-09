Building Sanzu
==============

The building and packaging process uses Docker containers.


* **Debian bullseye** : `make debian` creates 3 .deb packages in target/debian
* **Almalinux 8** : `make almalinux8` creates .rpm in target/generate-rpm
* **Windows** : `make windows` client only, zip file in target/windows


Two more options:
* **clean**:  `make clean` to cleanup the target directory
* **mrproper** : `make mrproper` to cleanup and removes all docker images
