const fs = require('fs');
const path = require('path');
const util = require('util');
const { exec: execNonPromise, spawn } = require('child_process');
const exec = util.promisify(execNonPromise);

const TOOL_IP = path.join(__dirname, 'tools/ip.sh');
const TOOL_PROXY = path.join(__dirname, 'tools/proxy.sh');

// log helper fns
const log = (...args) =>
  (global.Omegga ?? global.Logger).log(
    'wsl2binds'.underline,
    '>>'.green,
    ...args
  );
const info = (...args) =>
  (global.Omegga ?? global.Logger).log(
    'wsl2binds'.underline,
    '?>'.blue,
    ...args
  );
const error = (...args) =>
  (global.Omegga ?? global.Logger).error(
    'wsl2binds'.underline,
    '!>'.red,
    ...args
  );

module.exports = class Binder {
  constructor(omegga, _config, _store) {
    this.omegga = omegga;
  }

  setup() {
    const { ip, port } = this;
    if (this.child) {
      error('Error - attempting to re-setup before process is closed');
      return;
    }

    log(`UDP Proxy - Now forwarding to ${`${ip}:${port}`.yellow}`);
    // create the child process
    this.child = spawn(TOOL_PROXY, [ip, port], {
      cwd: path.join(__dirname, 'tools'),
      stdio: ['ignore', 'pipe', 'pipe'],
      shell: true,
    });
    this.pid = null;

    const handleLine = data => {
      data = data.trim();

      const isPid = data.match(/^pid = (\d+)$/);
      const isClientOpen = data.match(
        /^client (?<ip>\d+\.\d+\.\d+\.\d+:\d+) -> 0\.0\.0\.0:(?<port>\d+)$/
      );
      const isClientClose = data.match(
        /^client (?<ip>\d+\.\d+\.\d+\.\d+:\d+) -> closed$/
      );
      const isStart = data.match(/^Listening on 0\.0\.0\.0:\d+$/);

      if (isPid && !this.pid) {
        this.pid = isPid[1];
        info('UDP Proxy - PID is', (this.pid + '').yellow);
      } else if (isClientOpen) {
        log(
          'Joining client',
          isClientOpen.groups.ip.yellow,
          '->',
          isClientOpen.groups.port.yellow
        );
      } else if (isClientClose) {
        // no need to log timed out clients
        // log('Timed out client', isClientClose.groups.ip.yellow);
      } else if (isStart) {
        log('UDP Proxy - Listen server started');
      } else if (data.length > 0) {
        log(`stdout: ${data}`);
      }
    };

    // handle output
    this.child.stdout.on('data', data => {
      data.toString().split('\n').forEach(handleLine);
    });

    // print error to console
    this.child.stderr.on('data', data => {
      error(`stderr: ${data}`);
    });

    this.child.on('close', code => {
      log(`UDP Proxy - Process exited with code ${(code + '').yellow}`);
      this.child = undefined;

      if (!this.closed) {
        info('Restarting proxy in', '5 seconds'.yellow);
        setTimeout(() => {
          this.setup();
        }, 5000);
      }
    });
  }

  async init() {
    this.closed = false;

    // make sure this plugin can run
    if (!this.omegga.config) {
      return error(
        'Omegga is outdated and missing required features for this plugin to operate'
      );
    }

    // check if this is wsl2 (/run/WSL/ exists on wsl2 instances)
    const isWSL2 = fs.existsSync('/run/WSL');
    if (!isWSL2) {
      return info('Not in WSL2 environment! This plugin is redundant!');
    }
    log('WSL2 detected');

    // get the IP from a handy dandy smallguy one liner sh script
    let { stdout: ip, stderr } = await exec(TOOL_IP, { shell: true });

    // remove newline from IP
    ip = ip.trim();

    // make sure that the output is an IP
    if (!ip || !ip.match(/^\d+\.\d+\.\d+\.\d+$/)) {
      return error('Could not find IP', ip, stderr);
    }

    const { port: webPort } = this.omegga.config.omegga;
    this.ip = ip;

    info(
      `Please run the following command in ${
        'Windows Powershell'.brightYellow
      } as ${'Administrator'.brightYellow} to access the ${'Web UI'.green}:`
    );
    info(
      `netsh interface portproxy add v4tov4 listenport=${webPort} listenaddress=0.0.0.0 connectport=${webPort} connectaddress=${ip}`
        .grey
    );
    info(
      `Or simply connect to the Web UI on the same PC with ${
        `https://${ip}:${webPort}`.green
      }`
    );

    // get config from omegga config
    const { port } = this.omegga.config.server;
    this.port = port;

    this.setup();
  }

  // cleanup by killing the proxy child process
  async stop() {
    this.closed = true;
    if (this.pid) {
      log('Killing proxy process', this.pid);
      spawn('kill', ['-9', this.pid]);
    }
    if (this.child) {
      this.child.kill('SIGINT');
    }
  }
};
