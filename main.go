package main

import (
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/charmbracelet/bubbles/list"
	"github.com/charmbracelet/bubbles/textarea"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/fsnotify/fsnotify"
)

var (
	focusedStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("205"))
	blurredStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("240"))
	focusedButton = focusedStyle.Render("[ Copy ]")
	blurredButton = blurredStyle.Render("[ Copy ]")
)

type sessionState uint

const (
	fileTreeView = iota
	textAreaView
	acceptView
)

type node struct {
	path           string
	isDir          bool
	children       []*node
	expanded       bool
	selected       bool
	parent         *node
	childrenLoaded bool
}

func (n *node) toggleSelect(on bool) {
	n.selected = on
	if n.isDir {
		for _, c := range n.children {
			c.toggleSelect(on)
		}
	}
}

func loadChildren(n *node, watcher *fsnotify.Watcher) {
	files, err := os.ReadDir(n.path)
	if err != nil {
		return
	}
	n.children = nil
	for _, f := range files {
		childPath := filepath.Join(n.path, f.Name())
		child := &node{
			path:   childPath,
			isDir:  f.IsDir(),
			parent: n,
		}
		n.children = append(n.children, child)
		if child.isDir {
			watcher.Add(childPath)
		}
	}
	n.childrenLoaded = true
}

type item struct {
	node  *node
	depth int
}

func (i item) Title() string       { return filepath.Base(i.node.path) }
func (i item) Description() string { return i.node.path }
func (i item) FilterValue() string { return filepath.Base(i.node.path) }

type customDelegate struct {
	list.DefaultDelegate
}

func (d customDelegate) Render(w io.Writer, lm list.Model, index int, listItem list.Item) {
	i, ok := listItem.(item)
	if !ok {
		return
	}

	name := filepath.Base(i.node.path)
	prefix := strings.Repeat("  ", i.depth)
	var symbol string
	if i.node.isDir {
		if i.node.expanded {
			symbol = "📂 "
		} else {
			symbol = "📁 "
		}
	} else {
		symbol = "📄 "
	}
	str := prefix + symbol + name

	var checkbox string
	if i.node.selected {
		checkbox = "[x]"
	} else {
		checkbox = "[ ]"
	}
	checkboxStyle := lipgloss.NewStyle().Width(3)
	checkboxStr := checkboxStyle.Render(checkbox)

	listItemStyle := lipgloss.NewStyle().Width(lm.Width() - 3)
	if index == lm.Index() {
		listItemStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("170")).Inherit(listItemStyle)
	}
	listItemStr := listItemStyle.Render(str)

	fmt.Fprint(w, lipgloss.JoinHorizontal(lipgloss.Center, listItemStr, checkboxStr))
}

type (
	fsEventMsg fsnotify.Event
	fsErrMsg   error
)

type model struct {
	list      list.Model
	textarea  textarea.Model
	watcher   *fsnotify.Watcher
	root      *node
	flatItems []list.Item
	focus     sessionState
	err       error
	prompt    string
	width     int
	height    int
	quitting  bool
}

func newModel(path string) model {
	abspath, err := filepath.Abs(path)
	if err != nil {
		return model{
			err: err,
		}
	}
	watcher, err := fsnotify.NewWatcher()
	root := &node{path: abspath, isDir: true, expanded: true}
	watcher.Add(abspath)
	loadChildren(root, watcher)
	flat := flatten(root)
	ld := list.NewDefaultDelegate()
	ld.SetSpacing(0)
	ld.SetHeight(1)
	d := customDelegate{ld}
	l := list.New(flat, d, 0, 0)
	l.Title = "File Tree"
	l.SetShowStatusBar(false)
	l.SetFilteringEnabled(true)
	l.SetShowHelp(false)
	l.InfiniteScrolling = true
	ta := textarea.New()
	ta.Placeholder = "Enter your task here..."
	ta.CharLimit = 0
	return model{
		list:      l,
		textarea:  ta,
		watcher:   watcher,
		root:      root,
		flatItems: flat,
		focus:     fileTreeView,
		err:       err,
	}
}

func flatten(root *node) []list.Item {
	var flat []list.Item
	var recurse func(*node, int)
	recurse = func(n *node, d int) {
		flat = append(flat, item{n, d})
		if n.expanded {
			for _, c := range n.children {
				recurse(c, d+1)
			}
		}
	}
	for _, c := range root.children {
		recurse(c, 0)
	}
	return flat
}

func (m model) Init() tea.Cmd {
	return tea.Batch(watchCmd(m.watcher), textarea.Blink)
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	var cmds []tea.Cmd
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		m.list.SetSize(msg.Width/2, msg.Height-4)
		m.textarea.SetWidth(msg.Width/2 - 2)
		m.textarea.SetHeight(msg.Height - 10)
		return m, nil
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c", "q":
			m.quitting = true
			return m, tea.Quit
		}
		if m.focus == fileTreeView {
			// don't expand/select entries if user is trying to edit the filter
			if !m.list.SettingFilter() {
				switch msg.String() {
				case "enter":
					if sel, ok := m.list.SelectedItem().(item); ok {
						if sel.node.isDir {
							curPath := sel.node.path
							sel.node.expanded = !sel.node.expanded
							if sel.node.expanded && !sel.node.childrenLoaded {
								loadChildren(sel.node, m.watcher)
							}
							m.flatItems = flatten(m.root)
							m.list.SetItems(m.flatItems)
							for idx, it := range m.flatItems {
								if it.(item).node.path == curPath {
									m.list.Select(idx)
									break
								}
							}
						}
					}
				case " ":
					if sel, ok := m.list.SelectedItem().(item); ok {
						on := !sel.node.selected
						sel.node.toggleSelect(on)
					}
				case "tab":
					m.focus = textAreaView
					cmds = append(cmds, m.textarea.Focus())
				}
			}
			m.list, cmd = m.list.Update(msg)
			cmds = append(cmds, cmd)
		} else if m.focus == textAreaView {
			switch msg.String() {
			case "tab":
				m.focus = acceptView
				m.textarea.Blur()
			}
			m.textarea, cmd = m.textarea.Update(msg)
			cmds = append(cmds, cmd)
		} else if m.focus == acceptView {
			switch msg.String() {
			case "enter":
				m.prompt = m.generatePrompt()
				return m, tea.Quit
			case "tab":
				m.focus = fileTreeView
			}
		}
	case fsEventMsg:
		ev := fsnotify.Event(msg)
		dir := filepath.Dir(ev.Name)
		node := findNode(m.root, dir)
		if node != nil && node.expanded && ev.Op != fsnotify.Write {
			loadChildren(node, m.watcher)
			m.flatItems = flatten(m.root)
			m.list.SetItems(m.flatItems)
		}
		cmds = append(cmds, watchCmd(m.watcher))
	case fsErrMsg:
		m.err = error(msg)
		cmds = append(cmds, watchCmd(m.watcher))
	default:
		var cmd2 tea.Cmd
		m.list, cmd = m.list.Update(msg)
		cmds = append(cmds, cmd)
		m.textarea, cmd2 = m.textarea.Update(msg)
		cmds = append(cmds, cmd2)
	}
	return m, tea.Batch(cmds...)
}

func (m model) View() string {
	if m.quitting {
		return "Bye!\n"
	}
	left := lipgloss.NewStyle().Width(m.width / 2).Height(m.height - 4).Render(m.list.View())
	rightTop := "User Request:"
	rightMid := m.textarea.View()
	rightBot := blurredButton
	if m.focus == acceptView {
		rightBot = focusedButton
	}
	right := lipgloss.NewStyle().Width(m.width / 2).Height(m.height - 4).PaddingLeft(2).Render(rightTop + "\n" + rightMid + "\n\n" + rightBot)
	return lipgloss.JoinHorizontal(lipgloss.Top, left, right) + "\nPress q to quit."
}

func watchCmd(w *fsnotify.Watcher) tea.Cmd {
	return func() tea.Msg {
		select {
		case ev := <-w.Events:
			return fsEventMsg(ev)
		case err := <-w.Errors:
			return fsErrMsg(err)
		}
	}
}

func findNode(n *node, path string) *node {
	if n.path == path {
		return n
	}
	if n.childrenLoaded {
		for _, c := range n.children {
			if f := findNode(c, path); f != nil {
				return f
			}
		}
	}
	return nil
}

func (m model) generatePrompt() string {
	var sb strings.Builder
	sb.WriteString("<file_tree>\n")
	sb.WriteString(generateFileTree(m.root))
	sb.WriteString("</file_tree>\n")
	selectedFiles := []string{}
	var collect func(n *node)
	collect = func(n *node) {
		if n.selected && !n.isDir {
			selectedFiles = append(selectedFiles, n.path)
		}
		if n.childrenLoaded {
			for _, c := range n.children {
				collect(c)
			}
		}
	}
	collect(m.root)
	for _, p := range selectedFiles {
		sb.WriteString("<file>\n<file_path>" + p + "</file_path>\n<file_content>\n")
		b, err := os.ReadFile(p)
		var content string
		if err != nil || strings.Contains(string(b), "\x00") {
			content = "[Binary file]"
		} else {
			content = string(b)
		}
		sb.WriteString(content)
		sb.WriteString("\n</file_content>\n</file>\n")
	}
	sb.WriteString("<user_request>\n" + m.textarea.Value() + "\n</user_request>")
	return sb.String()
}

func generateFileTree(root *node) string {
	var sb strings.Builder
	children := []*node{}
	for _, c := range root.children {
		if c.selected || hasSelected(c) {
			children = append(children, c)
		}
	}
	for i, c := range children {
		isLast := i == len(children)-1
		sb.WriteString(generateTreeRec(c, "", isLast))
	}
	return sb.String()
}

func generateTreeRec(n *node, prefix string, isLast bool) string {
	var s string
	name := filepath.Base(n.path)
	if isLast {
		s = prefix + "└── " + name + "\n"
		prefix += "    "
	} else {
		s = prefix + "├── " + name + "\n"
		prefix += "│   "
	}
	children := []*node{}
	for _, c := range n.children {
		if c.selected || hasSelected(c) {
			children = append(children, c)
		}
	}
	for i, c := range children {
		isLastChild := i == len(children)-1
		s += generateTreeRec(c, prefix, isLastChild)
	}
	return s
}

func hasSelected(n *node) bool {
	if n.selected && !n.isDir {
		return true
	}
	if n.childrenLoaded {
		for _, c := range n.children {
			if hasSelected(c) {
				return true
			}
		}
	}
	return false
}

func main() {
	path := flag.String("path", ".", "path to directory to open")
	flag.Parse()
	p := tea.NewProgram(newModel(*path), tea.WithAltScreen())
	fm, err := p.Run()
	if err != nil {
		fmt.Println("Error:", err)
		os.Exit(1)
	}
	if m, ok := fm.(model); ok && m.prompt != "" {
		cmd := exec.Command("pbcopy")
		cmd.Stdin = strings.NewReader(m.prompt)
		_ = cmd.Run()
	}
	if m, ok := fm.(model); ok {
		m.watcher.Close()
	}
}
