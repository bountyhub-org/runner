// Package expr contains expression level helpers
package expr

import (
	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
	"github.com/google/cel-go/cel"
	"github.com/google/cel-go/common/types/ref"
	"github.com/google/cel-go/interpreter"
)

var celEnv *cel.Env

func init() {
	project := &jobexecutionv1connect.Project{}
	workflow := &jobexecutionv1connect.Workflow{}
	revision := &jobexecutionv1connect.Revision{}
	stepContext := &StepContext{}
	e, err := cel.NewEnv(
		cel.Types(
			project,
			workflow,
			revision,
			stepContext,
		),

		// Job context variables
		cel.VariableWithDoc("id", cel.StringType, "The unique identifier of the job"),
		cel.Variable("name", cel.StringType),
		cel.Variable("vars", cel.MapType(cel.StringType, cel.StringType)),
		cel.Variable("secrets", cel.MapType(cel.StringType, cel.StringType)),
		cel.Variable("env", cel.MapType(cel.StringType, cel.StringType)),
		cel.Variable("project", cel.ObjectType(string(project.ProtoReflect().Descriptor().FullName()))),
		cel.Variable("workflow", cel.ObjectType(string(workflow.ProtoReflect().Descriptor().FullName()))),
		cel.Variable("revision", cel.ObjectType(string(revision.ProtoReflect().Descriptor().FullName()))),
		cel.Variable("inputs", cel.MapType(cel.StringType, cel.DynType)),
		cel.Variable("steps", cel.ListType(cel.ObjectType(stepContext.TypeName()))),

		// Control variables
		cel.Variable("ok", cel.BoolType),
		cel.Variable("always", cel.BoolType),
	)
	if err != nil {
		panic(err)
	}
	celEnv = e
}

type Engine struct {
	env  *cel.Env
	data *jobData
}

var _ interpreter.Activation = (*jobData)(nil)

type jobData struct {
	id       string
	name     string
	vars     map[string]string
	secrets  map[string]string
	env      map[string]string
	project  *jobexecutionv1connect.Project
	workflow *jobexecutionv1connect.Workflow
	revision *jobexecutionv1connect.Revision
	inputs   map[string]any
	steps    []*StepContext
	scans    map[string]*jobexecutionv1connect.ArtifactsContext

	ok bool
}

// Parent implements [interpreter.Activation].
func (j *jobData) Parent() interpreter.Activation {
	return nil
}

// ResolveName implements [interpreter.Activation].
func (j *jobData) ResolveName(name string) (any, bool) {
	switch name {
	case "id":
		return j.id, true
	case "name":
		return j.name, true
	case "vars":
		return j.vars, true
	case "secrets":
		return j.secrets, true
	case "env":
		return j.env, true
	case "project":
		return j.project, true
	case "workflow":
		return j.workflow, true
	case "revision":
		return j.revision, true
	case "inputs":
		return j.inputs, true
	case "steps":
		return j.steps, true
	case "ok":
		return j.ok, true
	case "always":
		return true, true
	case "scans":
		return j.scans, true
	default:
		return nil, false
	}
}

type StepStatus int

const (
	_ StepStatus = iota
	StepStatusSucceeded
	StepStatusFailed
	StepStatusSkipped
)

type StepOutcome int

const (
	_ StepOutcome = iota
	StepOutcomeSucceeded
	StepOutcomeFailed
	StepOutcomeCancelled
)

var _ ref.Type = (*StepContext)(nil)

type StepContext struct {
	Status  StepStatus
	Outcome StepOutcome
}

// HasTrait implements [ref.Type].
func (s *StepContext) HasTrait(trait int) bool {
	return false
}

// TypeName implements [ref.Type].
func (s *StepContext) TypeName() string {
	return "expr.StepContext"
}

func NewEngine(jobContext *jobexecutionv1connect.ResolveJobResponse) *Engine {
	steps := make([]*StepContext, 0, len(jobContext.Steps))
	for range jobContext.Steps {
		steps = append(steps, &StepContext{})
	}

	inputs := make(map[string]any)
	for k, v := range jobContext.Inputs {
		switch v := v.Value.(type) {
		case *jobexecutionv1connect.InputValue_String_:
			inputs[k] = v.String_
		case *jobexecutionv1connect.InputValue_Bool:
			inputs[k] = v.Bool
		default:
			panic("unknown type")
		}
	}

	return &Engine{
		env: celEnv,
		data: &jobData{
			id:       jobContext.Id,
			name:     jobContext.Name,
			vars:     jobContext.Vars,
			secrets:  jobContext.Secrets,
			env:      jobContext.Env,
			project:  jobContext.Project,
			workflow: jobContext.Workflow,
			revision: jobContext.Revision,
			inputs:   inputs,
			steps:    steps,
			scans:    jobContext.ScanHistories,
			ok:       true,
		},
	}
}

func (e *Engine) Eval(expr string) (ref.Val, error) {
	ast, issues := e.env.Parse(expr)
	if issues != nil && issues.Err() != nil {
		return nil, issues.Err()
	}
	checked, issues := e.env.Check(ast)
	if issues != nil && issues.Err() != nil {
		return nil, issues.Err()
	}
	prg, err := e.env.Program(checked)
	if err != nil {
		return nil, err
	}
	out, _, err := prg.Eval(e.data)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (e *Engine) UpdateStateFromStep(idx int, step *StepContext) {
	if idx < 0 {
		panic("index must be non-negative")
	}

	if idx >= len(e.data.steps) {
		panic("index out of bounds")
	}

	e.data.steps[idx] = step

	if e.data.ok && step.Outcome != StepOutcomeSucceeded {
		e.data.ok = false
	}
}
