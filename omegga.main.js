const fs = require('fs');
const path = require('path');
const util = require('util');
const {exec: execNonPromise, spawn} = require('child_process');
const exec = util.promisify(execNonPromise);

const TOOL_IP = path.join(__dirname, 'tools/ip.sh');
const TOOL_PROXY = path.join(__dirname, 'tools/proxy.sh');

// log helper fns
const log = (...args) => Omegga.log('wsl2binds'.underline, '>>'.green, ...args);
const info = (...args) => Omegga.log('wsl2binds'.underline, '?>'.blue, ...args);
const error = (...args) => Omegga.error('wsl2binds'.underline, '!>'.red, ...args);

module.exports = class Binder {
  constructor(omegga, _config, _store) {
    this.omegga = omegga;
  }

  async init() {
    // make sure this plugin can run
    if (!this.omegga.config) {
      return error('Omegga is outdated and missing required features for this plugin to operate');
    }

    // check if this is wsl2 (/run/WSL/ exists on wsl2 instances)
    const isWSL2 = fs.existsSync('/run/WSL');
    if (!isWSL2) {
      return info('Not in WSL2 environment! This plugin is redundant!');
    }
    log('WSL2 detected');


    // get the IP from a handy dandy smallguy one liner sh script
    let {stdout: ip, stderr} = await exec(TOOL_IP, {shell: true});

    // remove newline from IP
    ip = ip.trim();

    // make sure that the output is an IP
    if (!ip || !ip.match(/^\d+\.\d+\.\d+\.\d+$/)) {
      return error('Could not find IP', ip, stderr);
    }

    // get config from omegga config
    const { port } = this.omegga.config.server;

    log(`UDP Proxy - Forwarding to ${ip}:${port}`);
    // create the child process
    this.child = spawn(TOOL_PROXY, [ip, port], {
      cwd: path.join(__dirname, 'tools'),
      stdio: ['ignore', 'pipe', 'pipe'],
      shell: true,
    });

    // handle output
    this.child.stdout.on('data', data => {
      data = data.toString().trim();

      const isPid = data.match(/^pid = (\d+)$/);
      const isClient = data.match(/^client (?<ip>\d+\.\d+\.\d+\.\d+:\d+) -> 0\.0\.0\.0:(?<port>\d+)$/);
      const isStart = data.match(/^Listening on 0\.0\.0\.0:\d+$/);

      if (isPid && !this.pid) {
        this.pid = isPid[1];
        info('UDP Proxy - PID is', this.pid);
      } else if (isClient) {
        log('Joining client ', isClient.groups.ip, '->', isClient.groups.port);
      } else if (isStart) {
        log('UDP Proxy - Listen server started');
      } else {
        log(`stdout: ${data}`);
      }
    });

    // print error to console
    this.child.stderr.on('data', data => {
      error(`stderr: ${data}`);
    });

    this.child.on('close', (code) => {
      log(`UDP Proxy - Exited with code ${code}`);
      this.child = undefined;
    });
  }

  // cleanup by killing the proxy child process
  async stop() {
    if (this.pid) {
      log('Killing proxy process', this.pid);
      spawn('kill', ['-9', this.pid]);
    }
    if (this.child) {
      this.child.kill('SIGINT');
    }
  }
};
