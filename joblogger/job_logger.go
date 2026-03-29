package joblogger

type Client interface{}

type JobLogger struct{}

func (l *JobLogger) Stdout(text string) {
}

func (l *JobLogger) Stderr(text string) {
}
