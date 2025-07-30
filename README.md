# localsend-rs

a cli for localsend

<div align="center">
  <video src="https://github.com/notjedi/localsend-rs/assets/30691152/6bedeb44-1dd8-4f72-8a8d-c1c2be715a26" type="video/mp4"></video>
</div>

the current idea for sending files is to make the server an Arc type in the bin
and spawn 2 tokio tasks - one to listen for messages from client and call
corresponding methods to send files and another one to do what we do now which
is to recv files.

a small todo: `use mem::take` when ever possible, to avoid clones.

## Features

- [x] Send files to other devices
- [x] Receive files from other devices
- [x] Cross-platform support (Linux, macOS, Windows)
- [x] Secure HTTPS communication
- [x] Device discovery via multicast
- [x] **Connection reset error handling with automatic retries**
- [x] **Request cancellation support (Ctrl+C)**
- [x] **Robust network error recovery**
- [ ] GUI interface (planned)
- [ ] Mobile app support (planned)

## Error Handling & Cancellation

### Connection Reset Handling
The application now includes robust error handling for network issues:
- **Automatic retries**: Failed connections are automatically retried up to 3 times
- **Connection reset detection**: Specifically handles connection reset, broken pipe, and network unreachable errors
- **Exponential backoff**: Retry delays increase progressively to avoid overwhelming the network
- **Detailed error reporting**: Clear error messages help identify the cause of failures

### Request Cancellation
- **Ctrl+C support**: Press Ctrl+C during file transfers to cancel operations
- **Graceful cancellation**: Both sending and receiving operations can be cancelled cleanly
- **Session cleanup**: Cancelled sessions are properly cleaned up on both client and server sides

### Network Resilience
- **Timeout handling**: Operations have appropriate timeouts to prevent hanging
- **Stream error recovery**: File receiving handles stream errors with retry logic
- **Progress tracking**: Failed transfers report how much data was successfully transferred

## Roadmap

- [x] receive files
- [x] send files
- [x] handle connection reset errors and cancel requests when sending and receiving files
- [ ] progress for sending files
- [x] pass config from bin to lib
- [ ] config file for device name, default port, etc
- [ ] Support protocol `v2`
- [x] fix `Illegal SNI hostname received` from dart side
