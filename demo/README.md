Sanzu demo
==========

This demo shows how to use sanzu with a `client <-> broker <-> server`
configuration with TLS certificate authentication and browsing inside
a Docker container.

You need a functioning Docker installation with networking and 
port exposure.

A `server` container has `sanzu_broker` and `sanzu_server`. When a
client connects and get authenticated it starts a Xvfb session and
a firefox browser for the client.

The `client` containers is a `sanzu_client`, it will only work
in a X11 environment.

Usage
-----

```./run-demo.sh -h
Usage: ./run-demo.sh -l [ -p PROXY ] -c -d -h
  -h : shows usage
  -l : shows the command line for a local client instead of starting a containered one
  -c : cleanup containers and temporary files
  -d : debug mode
  -p PROXY: use a HTTP/HTTPs proxy inside the container
```


Notes
-----

Sound is not working for now
