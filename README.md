![Runner Logo](./assets/logo.jpeg)

# BountyHub Runner

The runner is the application that runs jobs from the BountyHub server. It listens to the new jobs associated with your account
and runs them on your machine. It is organized as a CLI application that can be run in the background, or on-demand.

More information about the BountyHub project can be found [here](https://docs.bountyhub.org/runner)

# Installation

## Download the binary

You can download the binary from the [releases page](https://github.com/bountyhub-org/runner/releases).

Extract packages and move the binary to the desired location.


## From source

### Build from source

You can build the runner from source by cloning the repository and running the following command:

```bash
cargo install --path crates/cli
mv ~/.cargo/bin/cli ~/.cargo/bin/runner
```

### Running the runner

To run the runner, you should provide a working directory. By default, it will be `./_work`. You can change it by providing the `--workdir` flag.

```bash
runner configure --token <your_token> --url <server_url> --name <unique_name> --workdir /path/to/workdir
```

## Docker

### Build your own docker image

The image for the runner is created as a base image that you can use to build your own runner. You can find the Dockerfile in the [images](./images) directory.

For example, you can create a Dockerfile like this:

```Dockerfile
FROM bountyhuborg/runner:0.7.0 AS runner

FROM ubuntu:24.04

WORKDIR /home/runner

COPY --from=runner /runner /home/runner/runner
COPY --from=runner /bh /usr/local/bin/bh

# Install additional dependencies
RUN apt-get update && apt-get install -y \
    git \
    curl \
    wget \
    jq \
    golang

# Install tooling
RUN go install -v github.com/projectdiscovery/httpx/cmd/httpx@latest \
    && go install -v github.com/projectdiscovery/subfinder/v2/cmd/subfinder@latest \
    && go install -v github.com/projectdiscovery/nuclei/v3/cmd/nuclei@latest \
    && go install -v github.com/tomnomnom/anew@latest \
    && go install github.com/lc/gau/v2/cmd/gau@latest
```

To build the image, you can run the following command:

```bash
docker build -t bh-runner .
```

### Run the image

To run the runner in a container, you first need to configure it. In order to preserve the configuration, you can use the volume mount.

```bash
mkdir runner # Create directory where you will store the configuration
cd runner # Change to the directory
docker run -v $(pwd):/home/runner bh-runner runner configure --token <your_token> --url <server_url> --name <unique_name> --workdir /home/runner
```

Once the configuration is done, you can see the `.runner` file present in the directory. You can now run the runner in the container.

```bash
docker run -v $(pwd):/home/runner bh-runner runner run
```

# Shell completions


To enable auto-completion, you can run the following command:

If you are not sure which shell you are using, you can run the following command:

```bash
echo $SHELL
```

You can run the following command to see what shells are available:

```bash
./runner completion --help
```

### Examples:

#### Bash

```bash
source <(./runner completion bash)
```

#### Zsh
```bash
source <(./runner completion zsh)
```

