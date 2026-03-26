// Package expr contains expression level helpers
package expr

import (
	"fmt"

	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
	"github.com/google/cel-go/cel"
	"github.com/google/cel-go/common/types"
	"github.com/google/cel-go/common/types/ref"
	"github.com/google/cel-go/interpreter"
)

var celEnv *cel.Env

func init() {
	project := &jobexecutionv1connect.Project{}
	workflow := &jobexecutionv1connect.Workflow{}
	revision := &jobexecutionv1connect.Revision{}
	artifactsContext := &jobexecutionv1connect.ArtifactsContext{}
	stepContext := &StepContext{}
	artifactsContextType := cel.ObjectType(string(artifactsContext.ProtoReflect().Descriptor().FullName()))
	e, err := cel.NewEnv(
		cel.Types(
			project,
			workflow,
			revision,
			artifactsContext,
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
		cel.Variable("scans", cel.MapType(cel.StringType, artifactsContextType)),

		// Control variables
		cel.Variable("ok", cel.BoolType),
		cel.Variable("always", cel.BoolType),

		cel.Function(
			"is_available",
			cel.MemberOverload(
				"artifacts_context_is_available_string",
				[]*cel.Type{artifactsContextType, cel.StringType},
				cel.BoolType,
				cel.BinaryBinding(func(lhs, rhs ref.Val) ref.Val {
					scan, ok := lhs.Value().(*jobexecutionv1connect.ArtifactsContext)
					if !ok {
						return types.NewErr("no such overload")
					}

					artifactName, ok := rhs.Value().(string)
					if !ok {
						return types.NewErr("no such overload")
					}

					artifact, found := scan.GetContexts()[artifactName]
					if !found || artifact == nil {
						return types.Bool(false)
					}

					return types.Bool(artifact.GetIsAvailable())
				}),
			),
		),
	)
	if err != nil {
		panic(err)
	}
	celEnv = e
}

type Engine struct {
	env  *cel.Env
	data *data
}

var _ interpreter.Activation = (*data)(nil)

type data struct {
	id       string
	name     string
	vars     map[string]string
	secrets  map[string]string
	env      map[string]string
	project  *jobexecutionv1connect.Project
	workflow *jobexecutionv1connect.Workflow
	revision *jobexecutionv1connect.Revision
	inputs   map[string]any
	steps    []StepContext
	scans    map[string]*jobexecutionv1connect.ArtifactsContext

	ok bool
}

// Parent implements [interpreter.Activation].
func (j *data) Parent() interpreter.Activation {
	return nil
}

// ResolveName implements [interpreter.Activation].
func (j *data) ResolveName(name string) (any, bool) {
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

func (s StepStatus) Validate() error {
	switch s {
	case StepStatusSucceeded, StepStatusFailed, StepStatusSkipped:
		return nil
	default:
		return fmt.Errorf("invalid step status: %v", s)
	}
}

type StepOutcome int

const (
	_ StepOutcome = iota
	StepOutcomeSucceeded
	StepOutcomeFailed
	StepOutcomeCancelled
)

func (o StepOutcome) Validate() error {
	switch o {
	case StepOutcomeSucceeded, StepOutcomeFailed, StepOutcomeCancelled:
		return nil
	default:
		return fmt.Errorf("invalid step outcome: %v", o)
	}
}

var _ ref.Type = (*StepContext)(nil)

type StepContext struct {
	Status  StepStatus
	Outcome StepOutcome
}

func (s *StepContext) IsZero() bool {
	return s.Status == 0 || s.Outcome == 0
}

func (s *StepContext) Validate() error {
	if s.IsZero() {
		return fmt.Errorf("step context is not done with status %v and outcome %v", s.Status, s.Outcome)
	}

	if err := s.Status.Validate(); err != nil {
		return fmt.Errorf("invalid step status: %w", err)
	}
	if err := s.Outcome.Validate(); err != nil {
		return fmt.Errorf("invalid step outcome: %w", err)
	}

	if s.Outcome == StepOutcomeCancelled && s.Status != StepStatusFailed {
		return fmt.Errorf("step context has invalid state with status %v and outcome %v", s.Status, s.Outcome)
	}
	if s.Outcome == StepOutcomeFailed && s.Status != StepStatusFailed {
		return fmt.Errorf("step context has invalid state with status %v and outcome %v", s.Status, s.Outcome)
	}

	return nil
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
		data: &data{
			id:       jobContext.Id,
			name:     jobContext.Name,
			vars:     jobContext.Vars,
			secrets:  jobContext.Secrets,
			env:      jobContext.Env,
			project:  jobContext.Project,
			workflow: jobContext.Workflow,
			revision: jobContext.Revision,
			inputs:   inputs,
			steps:    make([]StepContext, len(jobContext.Steps)),
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

func (e *Engine) UpdateStateFromStep(idx int, nextContext StepContext) error {
	if idx < 0 {
		return fmt.Errorf("index cannot be negative")
	}

	if idx >= len(e.data.steps) {
		return fmt.Errorf("index %d out of bounds for steps with length %d", idx, len(e.data.steps))
	}

	if err := nextContext.Validate(); err != nil {
		return fmt.Errorf("invalid step context: %w", err)
	}

	currentContext := e.data.steps[idx]
	if !currentContext.IsZero() {
		return fmt.Errorf("step %d is already done with state %v", idx, currentContext)
	}
	e.data.steps[idx] = nextContext

	if e.data.ok && currentContext.Outcome != StepOutcomeSucceeded {
		e.data.ok = false
	}

	return nil
}
